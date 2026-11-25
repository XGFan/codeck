#!/bin/sh
# The checks a push needs (instead of CI), for the pre-push hook and by hand:
#   scripts/check.sh           what a push to origin/main would bring
#   scripts/check.sh --all     every group, whatever changed
# Rust changes → fmt, clippy, tests (plus ccrpc's mock end-to-end tests when ccrpc changed); frontend
# changes → Prettier, typecheck, lint, vitest, E2E; docs only → nothing.
#
# A group that passed is remembered for the content it ran on, under <git-common-dir>/codeck-checks:
# the same content is not checked twice, so running this before a push makes the push's own check
# instant. That content is what the group reads (see the keys below), so a commit to the frontend
# alone leaves what Rust and ccrpc's end-to-end tests passed on as it was. It runs on the working
# tree, so a pass is remembered only when the tracked files are as committed.
#   scripts/check.sh --keys [rev]   prints each group's key at rev (HEAD): why a group runs again
#
# Stops what it started when interrupted, or when the process that ran it (git push) is gone.
set -u
root=$(git rev-parse --show-toplevel) || exit 1
cd "$root" || exit 1

me=check
all=false
hook=false
keys=""
list=""
while [ $# -gt 0 ]; do
  case $1 in
    --all) all=true ;;
    --keys) keys=${2:-HEAD}; [ $# -gt 1 ] && shift ;;
    # The pre-push hook: the files the push brings, one per line (removed when done).
    --hook) hook=true; me=pre-push; list=$2; shift ;;
    *) echo "用法：scripts/check.sh [--all | --keys [rev]]" >&2; exit 2 ;;
  esac
  shift
done

# What a group passed on: the tracked files it reads, Markdown aside. Rust and the frontend are told
# apart by what the other alone reads, so a path nobody listed counts for both; each also reads a
# few of the other's files (the bindings a Rust test compares, the fixture the iPhone tests share
# with the Host's, the schema a frontend test checks its recordings against). ccrpc's end-to-end
# tests build ccrpc alone: its crate, the parsing library it uses, and Cargo's files.
tab=$(printf '\t')
web_only='src/|e2e/|e2e-host/|public/|package\.json$|pnpm-lock\.yaml$|pnpm-workspace\.yaml$|(vite|playwright|playwright\.host)\.config\.ts$|tsconfig[^/]*\.json$|eslint\.config\.js$|(index|transcript|terminal)\.html$|\.env\.|\.prettier'
rust_only='crates/|src-tauri/|Cargo\.(toml|lock)$|\.cargo/'
key_of() {
  tree=$(git ls-tree -r "$2" | grep -vE '\.md$')
  only() { echo "$tree" | grep -E "$tab($1)"; }
  without() { echo "$tree" | grep -vE "$tab($1)"; }
  case $1 in
    rust) { without "$web_only|ios/"; only 'src/ipc/bindings\.ts$|ios/Tests/Fixtures/'; } ;;
    web) { without "$rust_only|ios/"; only 'crates/ccrpc/docs/protocol\.schema\.json$'; } ;;
    ccrpc) only 'crates/ccrpc/|crates/transcript/|crates/codeck-host/src/ccrpc/|Cargo\.(toml|lock)$|\.cargo/' ;;
  esac | git hash-object --stdin
}
if [ -n "$keys" ]; then
  for g in rust ccrpc web; do echo "$g $(key_of "$g" "$keys")"; done
  exit 0
fi

if $hook; then
  changed=$(cat "$list")
elif $all; then
  changed=$(git ls-files)
else
  base=$(git merge-base HEAD origin/main 2>/dev/null || true)
  if [ -n "$base" ]; then changed=$(git diff --name-only "$base" HEAD); else changed=$(git ls-files); fi
fi

log=$(mktemp -t codeck-check)
child=""
kill_tree() {
  for k in $(pgrep -P "$1" 2>/dev/null); do kill_tree "$k"; done
  kill "$1" 2>/dev/null
}
stop() { [ -n "$child" ] && kill_tree "$child"; }
trap 'rm -f "$log"; [ -n "$list" ] && rm -f "$list"' EXIT
trap 'stop; echo; echo "$me: 已中断。" >&2; exit 130' INT TERM HUP

# Markdown (README, spec, TODO…) never needs a check, wherever it lives.
has() { echo "$changed" | grep -vE '\.md$' | grep -qE "$1"; }
rust_re='^(crates/|src-tauri/|Cargo\.(toml|lock)$|\.cargo/)'
web_re='^(src/|e2e/|e2e-host/|package\.json$|pnpm-lock\.yaml$|(vite|playwright|playwright\.host)\.config\.ts$|tsconfig[^/]*\.json$|eslint\.config\.js$|(index|transcript)\.html$|\.env\.|\.prettier)'
rust=false; ccrpc=false; web=false
has "$rust_re" && rust=true
has '^(crates/ccrpc/|crates/codeck-host/src/ccrpc/)' && ccrpc=true
has "$web_re" && web=true
if ! $rust && ! $web; then
  echo "$me: 只有文档等改动，跳过检查。"
  exit 0
fi

clean=true
git diff --quiet HEAD -- || clean=false
if ! $clean && $hook; then
  echo "pre-push: 工作区有未提交的改动，检查跑的不是要推送的内容。提交或还原后再推，或 git push --no-verify。" >&2
  exit 1
fi

dir=$(git rev-parse --git-common-dir)/codeck-checks
mkdir -p "$dir"
find "$dir" -name 'ok-*' -mtime +14 -delete 2>/dev/null
passed() { $clean && [ -e "$dir/ok-$1-$(key_of "$1" HEAD)" ]; }
label() {
  case $1 in
    rust) echo "Rust（fmt、clippy、cargo test）" ;;
    ccrpc) echo "ccrpc mock e2e" ;;
    web) echo "前端（Prettier、typecheck、lint、vitest、E2E）" ;;
  esac
}

todo=""
plan=""
secs=0
known=true
for g in rust ccrpc web; do
  eval "\$$g" || continue
  if passed "$g"; then
    echo "$me: $(label "$g") 这份内容已通过，跳过。"
    continue
  fi
  todo="$todo $g"
  plan="$plan${plan:+、}$(label "$g")"
  if [ -r "$dir/secs-$g" ]; then secs=$((secs + $(cat "$dir/secs-$g"))); else known=false; fi
done
[ -n "$todo" ] || exit 0
if $known; then
  echo "$me: 将运行 ${plan}；上次共约 $(((secs + 59) / 60)) 分钟。"
else
  echo "$me: 将运行 ${plan}。"
fi
$clean || echo "$me: 工作区有未提交的改动，这次通过不记录。"

# Untracked files are not part of what is pushed (nor of the key), but the checks run on the working
# tree and take them along: say so when one lies where a group that runs looks.
extra=$(git ls-files --others --exclude-standard | grep -vE '\.md$')
loose=""
case " $todo " in *" rust "* | *" ccrpc "*) loose=$(echo "$extra" | grep -E "$rust_re") ;; esac
case " $todo " in *" web "*) loose="$loose
$(echo "$extra" | grep -E "$web_re")" ;; esac
loose=$(echo "$loose" | sed '/^$/d')
if [ -n "$loose" ]; then
  n=$(echo "$loose" | wc -l | tr -d ' ')
  echo "$me: $n 个未跟踪的文件会被一起检查，但不在推送的内容里：$(echo "$loose" | head -n 3 | tr '\n' ' ')" >&2
fi

parent=$PPID
step() {
  name=$1
  shift
  printf '%s: %s … ' "$me" "$name"
  "$@" >"$log" 2>&1 &
  child=$!
  while kill -0 "$child" 2>/dev/null; do
    # Whoever ran this (git push) is gone: nobody waits for the result.
    if [ "$parent" != 1 ] && ! kill -0 "$parent" 2>/dev/null; then
      stop
      exit 130
    fi
    sleep 1
  done
  wait "$child"
  status=$?
  child=""
  if [ "$status" = 0 ]; then
    echo ok
  else
    echo 失败
    tail -n 40 "$log" >&2
    if $hook; then
      echo "pre-push: $name 没有通过，未推送。" >&2
    else
      echo "check: $name 没有通过。" >&2
    fi
    exit 1
  fi
}

for g in $todo; do
  start=$(date +%s)
  case $g in
    rust)
      step "cargo fmt" cargo fmt --all -- --check
      step "clippy" cargo clippy --workspace --all-targets -- -D warnings
      step "cargo test" cargo test --workspace
      ;;
    ccrpc)
      # Each test has its own ToH, claude and ccrpc state, so they run side by side: 6 at a time takes about
      # 80 s where one at a time took 7.5 min. More leaves too little room when the machine is busy.
      step "ccrpc mock e2e" cargo test -p ccrpc --test e2e mock_ -- --ignored --test-threads=6
      ;;
    web)
      step "Prettier" pnpm format:check
      step "typecheck" pnpm typecheck
      step "lint" pnpm lint
      step "vitest" pnpm test
      # Own port: never reuses a dev server left running by another worktree.
      step "E2E" env E2E_PORT="${CODECK_PUSH_E2E_PORT:-1499}" pnpm test:e2e
      ;;
  esac
  if $clean; then
    : >"$dir/ok-$g-$(key_of "$g" HEAD)"
    echo $(($(date +%s) - start)) >"$dir/secs-$g"
  fi
done
