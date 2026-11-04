#!/usr/bin/env python3
"""Builds the 「现状」 pages of the CoDeck design canvas from `pnpm design:shots`.

Usage: build-canvas.py <manifest.json> <blobs.json> <canvas.json as read from the canvas> <out dir>

- manifest.json: target/design-shots/manifest.json (pages and shots, in order).
- blobs.json: `<shot id>-light|dark` → the canvas asset id each PNG was uploaded as.
- Writes <out dir>/project/now-<id>.dc.html (light and dark side by side) and <out dir>/project/canvas.json:
  the canvas retitled 「CoDeck 设计」, the 现状 pages first, earlier 现状 boards and notes replaced, every
  other page, board and note kept. Publish them to the canvas with `root` = <out dir> (see CLAUDE.md).
"""
import json, sys, os
manifest = json.load(open(sys.argv[1])); blobs = json.load(open(sys.argv[2])); canvas_in = sys.argv[3]
out = os.path.join(sys.argv[4], "project")
os.makedirs(out, exist_ok=True)
W, H, GAP, ROW = 1280, 800, 80, 920 + 120
BW = W * 2 + GAP
tpl = '''<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>{title}</title>
<script src="./support.js"></script>
</head>
<body>
<x-dc>
<helmet>
<style>
body{{margin:0}}
</style>
</helmet>
<div style="width: {bw}px; height: {h}px; display: flex; gap: {gap}px; background: #e7e7ea">
<img src="/_blob/{light}" alt="{title}（亮色）" style="width: {w}px; height: {h}px; display: block">
<img src="/_blob/{dark}" alt="{title}（暗色）" style="width: {w}px; height: {h}px; display: block">
</div>
</x-dc>
<script type="text/x-dc" data-dc-script data-props='{{"$preview":{{"width":{bw},"height":{h}}}}}'>
class Component extends DCLogic {{
renderVals() {{
return {{}};
}}
}}
</script>
</body>
</html>
'''
c = json.load(open(canvas_in))
c["title"] = "CoDeck 设计"
now_pages = [{"id": p["id"], "name": p["name"]} for p in manifest["pages"]]
c["pages"] = now_pages + [p for p in c["pages"] if not p["id"].startswith("now-")]
c["launch"] = {"view": "canvas", "page": "now-main"}
# drop previous 现状 boards / notes, then add
for k in [k for k, v in c["boards"].items() if k.startswith("now-")]:
    del c["boards"][k]
c["order"] = [k for k in c["order"] if not k.startswith("now-")]
for k in [k for k in c["notes"] if k.startswith("now-")]:
    del c["notes"][k]
written = []
rows = {}
for s in manifest["shots"]:
    name = f"now-{s['id']}.dc.html"
    i = rows.get(s["page"], 0); rows[s["page"]] = i + 1
    c["boards"][name] = {"x": 0, "y": i * ROW, "w": BW, "h": H, "title": f"{s['title']} · 亮 / 暗", "page": s["page"]}
    c["order"].append(name)
    open(os.path.join(out, name), "w").write(tpl.format(title=s["title"], bw=BW, w=W, h=H, gap=GAP, light=blobs[s["id"] + "-light"], dark=blobs[s["id"] + "-dark"]))
    written.append("project/" + name)
for p in manifest["pages"]:
    c["notes"][f"now-title-{p['id']}"] = {"x": 0, "y": -300, "text": f"{p['name']}（实际界面截图）", "kind": "title1", "maxW": BW, "page": p["id"]}
    c["notes"][f"now-how-{p['id']}"] = {"x": BW + 80, "y": 0, "w": 480, "fill": "gray", "size": "m", "page": p["id"],
        "text": f"现状页 = 当前代码的截图（mock 数据），不在这里改。\n\n更新：仓库里跑 pnpm design:shots，再把 target/design-shots 同步到画布（见 CLAUDE.md「设计稿」）。\n\n新方案画成可编辑画板，放在对应主题的方案页里；采用的标「✓ 采用」。\n\n截图于 {manifest.get('at', '')}"}
json.dump(c, open(os.path.join(out, "canvas.json"), "w"), ensure_ascii=False, indent=2)
print(len(written), "boards")
print(json.dumps({w: w for w in written}, ensure_ascii=False))
