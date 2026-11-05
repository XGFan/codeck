//! Background tasks: what the plugin and the transcript report, and stopping one from `/tasks`.

use super::*;

impl Engine {
    /// A background task the plugin saw start: Bash `run_in_background`, an async Agent, a
    /// Workflow (from the tool's response).
    pub(super) fn note_background_launch(&mut self, tool_use_id: &str, i: &Value) {
        let (resp, input) = (&i["tool_response"], &i["tool_input"]);
        let text = |v: &Value| v.as_str().map(str::to_string);
        let task = match i["tool_name"].as_str() {
            Some("Bash") => text(&resp["backgroundTaskId"]).map(|id| (id, TaskKind::Shell)),
            Some("Agent" | "Task") if resp["isAsync"] == true => {
                text(&resp["agentId"]).map(|id| (id, TaskKind::Agent))
            }
            Some("Workflow") => text(&resp["taskId"]).map(|id| (id, TaskKind::Workflow)),
            _ => None,
        };
        let Some((id, kind)) = task else { return };
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
            command: (kind == TaskKind::Shell)
                .then(|| text(&input["command"]))
                .flatten(),
            status: "running".into(),
        });
    }

    /// Stop's `background_tasks`: running tasks ccrpc did not see start are added.
    pub(super) fn note_running_tasks(&mut self, bg: &Value) {
        for t in bg.as_array().into_iter().flatten() {
            let Some(id) = t["id"].as_str() else { continue };
            if self.hook_tasks.iter().any(|h| h.id == id)
                || self
                    .tailer
                    .as_ref()
                    .is_some_and(|tl| tl.tasks().iter().any(|h| h.id == id))
            {
                continue;
            }
            let kind = match t["type"].as_str() {
                Some("subagent") => TaskKind::Agent,
                Some("shell") => TaskKind::Shell,
                Some("workflow") => TaskKind::Workflow,
                _ => continue,
            };
            self.hook_tasks.push(BgTask {
                id: id.to_string(),
                kind,
                tool_use_id: String::new(),
                description: t["description"].as_str().map(str::to_string),
                command: t["command"].as_str().map(str::to_string),
                status: t["status"].as_str().unwrap_or("running").to_string(),
            });
        }
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
        // The panel shows a shell by its command and an agent or a Workflow by its description.
        let needle = match task.kind {
            TaskKind::Shell => task.command.clone(),
            _ => task.description.clone(),
        }
        .unwrap_or_default();
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
                if !fits(&text) || !text.contains("running") {
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
