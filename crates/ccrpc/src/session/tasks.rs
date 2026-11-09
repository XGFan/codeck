//! Background tasks: what the plugin and the transcript report, and stopping one from `/tasks`.

use super::*;

/// A PostToolUse that started a background task of the Main Line: `(id, kind)`. One started inside
/// a Subagent Run (the hook carries its `agent_id`) is that run's: its result notification goes to
/// the run, so the Main Line would never see it end.
fn launched(i: &Value) -> Option<(String, TaskKind)> {
    if !i["agent_id"].is_null() {
        return None;
    }
    let resp = &i["tool_response"];
    let text = |v: &Value| v.as_str().map(str::to_string);
    match i["tool_name"].as_str() {
        Some("Bash") => text(&resp["backgroundTaskId"]).map(|id| (id, TaskKind::Shell)),
        Some("Agent" | "Task") if resp["isAsync"] == true => {
            text(&resp["agentId"]).map(|id| (id, TaskKind::Agent))
        }
        Some("Monitor") => text(&resp["taskId"]).map(|id| (id, TaskKind::Monitor)),
        Some("Workflow") => text(&resp["taskId"]).map(|id| (id, TaskKind::Workflow)),
        _ => None,
    }
}

/// Stop's `background_tasks` (Claude Code's running tasks) into `tasks`: running ones ccrpc did not
/// see start (`seen`: in the transcript) are added. A Monitor tool's task is listed as a `shell`
/// there; seen start (plugin or transcript), it keeps its kind. A teammate (Agent with `name`) is
/// listed as `teammate` and counts as a Subagent; its tool result does not say it runs in the
/// background, and it only goes idle between messages (SubagentStop each time) until shut down, so
/// it is known only from here. Claude Code's own monitors (`monitor`, e.g. an artifact's live
/// updates) are left out: `/tasks` does not show them and no notification ends them.
///
/// A task known only from an earlier Stop that this one no longer lists has ended (`completed`): no
/// notification ends a teammate. Without the list (`bg` not an array) nothing ends.
fn merge_stop_tasks(tasks: &mut Vec<BgTask>, seen: impl Fn(&str) -> bool, bg: &Value) {
    let Some(bg) = bg.as_array() else { return };
    for t in tasks.iter_mut() {
        if t.tool_use_id.is_empty()
            && t.status == "running"
            && !bg.iter().any(|b| b["id"].as_str() == Some(t.id.as_str()))
        {
            t.status = "completed".into();
        }
    }
    for t in bg {
        let Some(id) = t["id"].as_str() else { continue };
        if tasks.iter().any(|h| h.id == id) || seen(id) {
            continue;
        }
        let kind = match t["type"].as_str() {
            Some("subagent" | "teammate") => TaskKind::Agent,
            Some("shell") => TaskKind::Shell,
            Some("workflow") => TaskKind::Workflow,
            _ => continue,
        };
        tasks.push(BgTask {
            id: id.to_string(),
            kind,
            tool_use_id: String::new(),
            description: t["description"].as_str().map(str::to_string),
            command: t["command"].as_str().map(str::to_string),
            status: t["status"].as_str().unwrap_or("running").to_string(),
            idle: false,
        });
    }
}

impl Engine {
    /// A background task the plugin saw start: Bash `run_in_background`, an async Agent, a
    /// Monitor, a Workflow (from the tool's response).
    pub(super) fn note_background_launch(&mut self, tool_use_id: &str, i: &Value) {
        let (resp, input) = (&i["tool_response"], &i["tool_input"]);
        let text = |v: &Value| v.as_str().map(str::to_string);
        // TaskStop ends a task without a notification of its own (a shell's never comes).
        if i["tool_name"] == "TaskStop" {
            let id = text(&input["task_id"]).or_else(|| text(&input["shell_id"]));
            if let Some(id) = id
                && self
                    .tasks()
                    .iter()
                    .any(|t| t.id == id && t.status == "running")
            {
                self.set_task_status(&id, "stopped");
            }
            return;
        }
        let Some((id, kind)) = launched(i) else {
            return;
        };
        if self.hook_tasks.iter().any(|t| t.id == id) {
            return;
        }
        let description = text(&input["description"])
            .or_else(|| text(&resp["description"]))
            .or_else(|| text(&resp["summary"]))
            .or_else(|| text(&resp["workflowName"]));
        if self.hook_tasks.len() >= 256 {
            self.hook_tasks.remove(0);
        }
        self.hook_tasks.push(BgTask {
            id,
            kind,
            tool_use_id: tool_use_id.to_string(),
            description,
            command: matches!(kind, TaskKind::Shell | TaskKind::Monitor)
                .then(|| text(&input["command"]))
                .flatten(),
            status: "running".into(),
            idle: false,
        });
    }

    /// Stop's `background_tasks`: see [`merge_stop_tasks`].
    pub(super) fn note_running_tasks(&mut self, bg: &Value) {
        let tailer = self.tailer.as_ref();
        let seen = |id: &str| tailer.is_some_and(|tl| tl.tasks().iter().any(|h| h.id == id));
        merge_stop_tasks(&mut self.hook_tasks, seen, bg);
    }

    pub(super) fn set_task_status(&mut self, id: &str, status: &str) {
        match self.hook_tasks.iter_mut().find(|t| t.id == id) {
            Some(t) => t.status = status.to_string(),
            None => {
                // One ccrpc saw only in the transcript (started before it connected).
                if let Some(mut t) = self
                    .tailer
                    .as_ref()
                    .and_then(|tl| tl.tasks().iter().find(|t| t.id == id).cloned())
                {
                    t.status = status.to_string();
                    self.hook_tasks.push(t);
                }
            }
        }
    }

    /// Background tasks: what the plugin reported, over what the transcript recorded (tasks
    /// started before ccrpc connected).
    pub(super) fn tasks(&self) -> Vec<BgTask> {
        let mut out: Vec<BgTask> = self
            .tailer
            .as_ref()
            .map(|t| t.tasks().to_vec())
            .unwrap_or_default();
        for h in &self.hook_tasks {
            match out.iter_mut().find(|t| t.id == h.id) {
                Some(t) => {
                    t.status = h.status.clone();
                    if t.tool_use_id.is_empty() {
                        t.tool_use_id = h.tool_use_id.clone();
                    }
                }
                None => out.push(h.clone()),
            }
        }
        // Known only from Stop (a plugin's monitor is listed as a shell there): its notifications
        // say it is a Monitor. Stop calls a teammate between rounds `running` too: the Main Line
        // tells whether it is idle.
        if let Some(tl) = self.tailer.as_ref() {
            for t in &mut out {
                if t.kind == TaskKind::Shell && tl.monitors().contains(&t.id) {
                    t.kind = TaskKind::Monitor;
                }
                t.idle = t.kind == TaskKind::Agent
                    && t.status == "running"
                    && t.tool_use_id.is_empty()
                    && t.description
                        .as_deref()
                        .is_some_and(|d| tl.teammate_idle(d));
            }
        }
        out
    }

    /// Stops a running background task from Claude Code's `/tasks` panel.
    pub(super) fn stop_task(&mut self, id: &str) -> CmdResult {
        let task = self.tasks().into_iter().find(|t| t.id == id);
        let Some(task) = task else {
            return err("unknown_request", format!("没有后台任务 {id}"));
        };
        if task.status != "running" {
            return err(
                "bad_request",
                format!("后台任务 {id} 已经结束（{}）", task.status),
            );
        }
        // The panel's list shows a shell by its command and an agent, a Monitor or a Workflow by
        // its description; a Monitor's details show its command (`Script:`), not its description.
        let needle = match task.kind {
            TaskKind::Shell => task.command.clone(),
            _ => task.description.clone(),
        }
        .unwrap_or_default();
        let detail_needle = match task.kind {
            TaskKind::Monitor => task.command.clone().unwrap_or_default(),
            _ => needle.clone(),
        };
        let detail_needle = recognize::collapse_ws(&detail_needle);
        let needle = recognize::collapse_ws(&needle);
        if needle.is_empty() {
            return err(
                "unrecognized_screen",
                format!("后台任务 {id} 没有可在面板里比对的命令或描述"),
            );
        }
        let fits = |text: &str| recognize::collapse_ws(text).contains(&needle);
        self.slash("tasks")?;
        let Some((panel, s)) = self.wait_screen(Duration::from_secs(3), |_, s| {
            recognize::tasks_panel(&s.lines)
        }) else {
            return Err(self.unrecognized("stop_task", None));
        };
        match panel {
            recognize::TasksPanel::Details(text) => {
                let fits_details = |t: &str| {
                    fits(t)
                        || (!detail_needle.is_empty()
                            && recognize::collapse_ws(t).contains(&detail_needle))
                };
                if !fits_details(&text) || !text.contains("running") {
                    self.keys(key::ESC)?;
                    return Err(self.unrecognized("stop_task", Some(&s)));
                }
            }
            recognize::TasksPanel::List(rows) => {
                match recognize::task_row(&rows, &needle) {
                    Ok(_) => {}
                    Err(why) => {
                        self.keys(key::ESC)?;
                        self.emit(json!({"type": "unrecognized", "context": format!("stop_task：{why}"), "lines": s.lines}));
                        return err("unrecognized_screen", format!("stop_task：{why}"));
                    }
                };
                // Move one row at a time (group titles are skipped by the cursor) until the
                // cursor is on the target row.
                let mut moved = false;
                for _ in 0..=rows.len() {
                    let Some(recognize::TasksPanel::List(now)) = self
                        .screen()
                        .ok()
                        .and_then(|s| recognize::tasks_panel(&s.lines))
                    else {
                        break;
                    };
                    // Found afresh each time: a running row's timer may change between frames.
                    let (Some(cur), Ok(want)) = (
                        now.iter().position(|(_, sel)| *sel),
                        recognize::task_row(&now, &needle),
                    ) else {
                        break;
                    };
                    if cur == want {
                        moved = true;
                        break;
                    }
                    let before = self.conn.generation();
                    self.keys(if want > cur { key::DOWN } else { key::UP })?;
                    self.conn.wait_change(before, Duration::from_millis(500));
                }
                if !moved {
                    self.keys(key::ESC)?;
                    return Err(self.unrecognized("stop_task", None));
                }
            }
        }
        self.keys(b"x")?;
        // The panel shows it at once; Claude Code tells the transcript only on the next turn.
        let ended = self.wait_screen(Duration::from_secs(5), |_, s| match recognize::tasks_panel(
            &s.lines,
        ) {
            Some(recognize::TasksPanel::List(rows)) => {
                recognize::task_row(&rows, &needle).is_err().then_some(())
            }
            Some(recognize::TasksPanel::Details(t)) => (!t.contains("running")).then_some(()),
            // The panel closed: back at the input box (not replaced by some other dialog).
            None => recognize::input_text(s).map(|_| ()),
        });
        if self
            .screen()
            .ok()
            .and_then(|s| recognize::tasks_panel(&s.lines))
            .is_some()
        {
            self.keys(key::ESC)?;
        }
        if ended.is_none() {
            return err("timeout", format!("5 秒内面板上的后台任务 {id} 仍在运行"));
        }
        let tid = id.to_string();
        if let Some(t) = self.tailer.as_mut() {
            t.mark_stopped(&tid);
        }
        // Claude Code's notice (`killed`) reaches the plugin at once when the main line is idle,
        // else once the running tool ends; until then the task is `stopped`.
        let status_of = |e: &Engine| {
            e.tasks()
                .into_iter()
                .find(|t| t.id == tid)
                .map(|t| t.status)
        };
        if status_of(self).as_deref() == Some("running") {
            self.set_task_status(&tid, "stopped");
        }
        self.wait_until(Duration::from_secs(2), |e| {
            status_of(e).is_some_and(|s| s != "running" && s != "stopped")
        });
        Ok(json!({"status": status_of(self)}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subagent_runs_own_background_task_is_not_the_main_lines() {
        let bash = |agent: Value| {
            json!({"tool_name": "Bash", "agent_id": agent,
                "tool_response": {"backgroundTaskId": "bsaikv62p"}})
        };
        assert_eq!(
            launched(&bash(Value::Null)),
            Some(("bsaikv62p".into(), TaskKind::Shell))
        );
        assert_eq!(launched(&bash(json!("ae174d96c4d6404de"))), None);
        // A teammate's result does not say it runs in the background.
        let teammate = json!({"tool_name": "Agent", "tool_response":
            {"status": "teammate_spawned", "agentId": "aline-a-codeck-0a19", "name": "line-a-codeck"}});
        assert_eq!(launched(&teammate), None);
    }

    #[test]
    fn stop_lists_a_teammate_as_a_subagent_until_it_is_gone() {
        let hooked = BgTask {
            id: "ae174d96c4d6404de".into(),
            kind: TaskKind::Agent,
            tool_use_id: "toolu_1".into(),
            description: Some("ccrpc side".into()),
            command: None,
            status: "running".into(),
            idle: false,
        };
        let mut tasks = vec![hooked.clone()];
        let both = json!([
            {"id": "tbriefqlv", "type": "teammate", "status": "running", "description": "CoDeck side"},
            {"id": "ae174d96c4d6404de", "type": "subagent", "status": "running", "description": "ccrpc side"},
        ]);
        merge_stop_tasks(&mut tasks, |_| false, &both);
        assert_eq!(tasks.len(), 2);
        assert_eq!(
            (
                tasks[1].id.as_str(),
                tasks[1].kind,
                tasks[1].status.as_str()
            ),
            ("tbriefqlv", TaskKind::Agent, "running")
        );
        assert_eq!(tasks[1].description.as_deref(), Some("CoDeck side"));
        // A Stop without the list (StopFailure) ends nothing.
        merge_stop_tasks(&mut tasks, |_| false, &Value::Null);
        assert_eq!(tasks[1].status, "running");
        // Shut down: the next Stop no longer lists it. One seen start keeps waiting for its notification.
        merge_stop_tasks(&mut tasks, |_| false, &json!([]));
        assert_eq!(tasks[1].status, "completed");
        assert_eq!(tasks[0], hooked);
    }
}
