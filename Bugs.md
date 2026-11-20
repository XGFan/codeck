# Bugs

这里记录在 Claude Code 里发现、影响 CoDeck / ccrpc 的问题：现象、证据、复现方法，以及是否已经报给上游。实测细节同步记在 [crates/ccrpc/docs/probes.md](crates/ccrpc/docs/probes.md)。

## Code Mode 里从代码提问时，模型那条消息直到作答才写进 transcript

- 发现：2026-11-21
- 版本：2.1.327、2.1.329（Code Mode，即 `CLAUDE_CODE_EXPERIMENTAL_CODE_MODE=1`；Fable 5.2；bypass 模式）
- 状态：未报给上游

### 现象

模型的一条回复是「文字 + REPL（代码里 `await tools.AskUserQuestion(…)`）+ Poll」。提问出现后，TUI 照常显示文字和提问，但这条消息的文字、REPL 条目、`(dispatched #N)` 结果与 Poll 条目都不在 transcript 里，直到作答、Poll 的结果送达，才和代码里的调用（`isVirtual` 的条目）一起写出，时间戳仍是生成时刻。正常时这些条目在提问出现后约 1 秒内写出。

对 CoDeck 的影响：对话内容只从 transcript 来，提问期间提问卡片上方看不到模型说明为什么要问的那段文字，作答后才补上。如果 Claude Code 在作答前退出或被杀，这段文字就不会写进 transcript，之后 resume 也找不回来。

### 证据

- 真实使用：一个 `--resume` 过的 Session，提问出现在 17:01:49，transcript 从 17:01:17 起再没写过（文件修改时间不变，全文搜不到那段文字）；17:09:24 作答后，thinking、文字、REPL、`(dispatched #4)`（时间戳 17:01:49.811）、Poll 与虚拟的 AskUserQuestion 条目一次写出。
- 同样条件（版本、beta、bypass）下另外 7 次从代码提问都在提问后约 0.1 秒写出：同一 Session resume 之前的 2 次，另一个新开的 Opus 5.5 Session 的 5 次。
- 文字确实生成在提问之前：晚写的条目的时间戳（文字 17:26:08.437、REPL 17:26:09.489）早于提问。

### 复现

经 ccrpc 驱动（`ccrpc new --cwd <空目录> -- --model claude-fable-5-2 [--resume <id>]`），每轮发同一段提示词，要求模型在同一条回复里先写一段以标记开头的文字，再调用 REPL 运行 `await tools.AskUserQuestion(…)`，再调用 Poll。收到 `question` 事件后 30 秒内每秒检查 transcript 里有没有带该标记的 assistant 文字块，然后 `answer_question`。

| Session | 轮数 | 晚写 |
|---|---|---|
| 新开 | 10 | 0 |
| `--resume` 过 | 9 | 2（resume 后的第 1、第 2 轮） |

- 只在 `--resume` 过的 Session 里出现过（真实使用的那次也是），样本小，还不能断定 resume 是条件。
- 和条目顺序无关：晚写与正常的轮次里，`(dispatched #N)` 都有时在 Poll 前、有时在 Poll 后。
- 没找到 Claude Code 里的原因（只从外部观察）。

<details>
<summary>复现脚本（Python，参数：cwd、模型、轮数、日志文件，其后是传给 claude 的参数；环境变量 TP 指定 transcript 路径）</summary>

```python
import json, subprocess, sys, time, glob, os, threading, queue
cwd, model, rounds, out = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
extra = sys.argv[5:]
log = open(out, 'a')
def L(*a):
    s = time.strftime('%H:%M:%S') + ' ' + ' '.join(str(x) for x in a)
    log.write(s + '\n'); log.flush()
args = ['ccrpc', 'new', '--hidden', '--cwd', cwd, '--', '--model', model] + extra
p = subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
q = queue.Queue()
def rd():
    for line in p.stdout:
        try: q.put(json.loads(line))
        except Exception: pass
    q.put(None)
threading.Thread(target=rd, daemon=True).start()
def send(o):
    p.stdin.write(json.dumps(o, ensure_ascii=False) + '\n'); p.stdin.flush()
def wait(pred, timeout):
    end = time.time() + timeout
    while time.time() < end:
        try: m = q.get(timeout=1)
        except queue.Empty: continue
        if m is None: return None
        t = m.get('type')
        if t == 'choice' and m.get('layout') == 'trust_folder':
            send({'type':'answer_choice','request_id':m['request_id'],'option':'Yes, I trust this folder'})
        if t == 'session_changed': wait.sid = m.get('session_id')
        if pred(m): return m
    return 'timeout'
wait.sid = None
r = wait(lambda m: m.get('type') == 'state' and m.get('state') == 'idle', 90)
L('idle', r if not isinstance(r, dict) else '', 'sid', wait.sid)
def has_text(tp, mark):
    if not tp: return False
    for line in open(tp, encoding='utf-8', errors='replace'):
        if mark not in line or '"type":"assistant"' not in line: continue
        try: o = json.loads(line)
        except Exception: continue
        c = o.get('message', {}).get('content')
        if isinstance(c, list) and any(b.get('type') == 'text' and mark in b.get('text','') for b in c): return True
    return False
def tpath():
    if os.environ.get('TP'): return os.environ['TP']
    if not wait.sid: return None
    g = glob.glob(os.path.expanduser('~/.claude/projects/*/' + wait.sid + '.jsonl'))
    return g[0] if g else None
for i in range(rounds):
    mark = 'MARK%d%d' % (os.getpid() % 1000, i)
    prompt = (os.environ.get('PRE','') + '这是一次复现测试，请严格照做：在同一条回复里，先写一段以 %s 开头、约 200 字的中文正文（随便讲讲你对代码整理的看法），'
              '然后调用 REPL，代码只有一行：await tools.AskUserQuestion({questions:[{question:"继续下一轮吗？",header:"继续",multiSelect:false,options:[{label:"继续",description:"进行下一轮"},{label:"停止",description:"结束测试"}]}]})，'
              '再调用 Poll 等它的结果。收到回答后只回一句「好」，不要做别的。') % mark
    send({'type':'prompt','text':prompt})
    m = wait(lambda m: m.get('type') == 'question', 300)
    if not isinstance(m, dict): L('round', i, 'no question', m); break
    tq = time.time()
    seen_at = None
    for k in range(30):
        tp = tpath()
        if tp and has_text(tp, mark):
            seen_at = time.time() - tq; break
        time.sleep(1)
    L('round', i, 'from_code', m.get('from_code'), 'text_in_transcript_after_question_s', None if seen_at is None else round(seen_at,1))
    send({'type':'answer_question','request_id':m['request_id'],'answers':[{'options':['继续']}]})
    ans = time.time()
    tp = tpath()
    for k in range(20):
        if tp and has_text(tp, mark):
            L('round', i, 'text appeared after answer s', round(time.time()-ans,1)); break
        time.sleep(0.5)
    r = wait(lambda m: m.get('type') == 'turn_end', 300)
    L('round', i, 'turn_end', 'ok' if isinstance(r, dict) else r)
    time.sleep(2)
send({'type':'exit','close_pane':True})
wait(lambda m: m.get('type') == 'session_exit', 30)
L('done sid', wait.sid, 'transcript', tpath())
p.terminate()
```

</details>
