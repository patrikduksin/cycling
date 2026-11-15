#!/usr/bin/env bash
set -u

words=()
while (($#)); do
  case $1 in
    -R | --repo) shift ;;
    --dry-run | --help | -h) exit 0 ;;
    -*) ;;
    *) words+=("$1") ;;
  esac
  shift
done
[[ ${words[0]-} == pr && ${words[1]-} =~ ^(create|new)$ ]] || exit 0

session=${CLAUDE_CODE_SESSION_ID-}
[[ -n $session ]] || exit 0
files=()
for file in "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/projects/*/"$session".jsonl \
  "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/projects/*/"$session"/subagents/agent-*.jsonl; do
  [[ -f $file ]] && files+=("$file")
done
if ((${#files[@]} == 0)); then
  echo "pr-gates: no transcript for session $session; could not check /deslop and /no-comments." >&2
  exit 0
fi

read -r -d '' gates_run <<'JQ' || true
def gate(pattern): capture(pattern) | .gate;
def typed_command: gate("<command-message>(pstack:)?(?<gate>deslop|no-comments)</command-message>");
def launched_skill:
  .content | if type == "array" then map(.text? // "") | join("") else . end
  | strings | gate("^Launching skill: (pstack:)?(?<gate>deslop|no-comments)$");
def reviewer_spawn:
  select(.name == "Agent" and .input.subagent_type == "pstack:comment-sicko") | "no-comments";

fromjson? | select(.type == "user" or .type == "assistant") | .message.content
| if type == "string" then typed_command
  else .[]?
    | if .type == "tool_result" then launched_skill
      elif .type == "tool_use" then reviewer_spawn
      else empty end
  end
JQ
ran=$(jq -rR "$gates_run" "${files[@]}" | sort -u)

missing=()
grep -qx deslop <<<"$ran" || missing+=("/deslop")
grep -qx no-comments <<<"$ran" || missing+=("/no-comments")
((${#missing[@]})) || exit 0

if ((${#missing[@]} == 2)); then
  gates="/deslop and /no-comments have"
else
  gates="${missing[0]} has"
fi
echo "gh pr create refused: $gates not run in this session. Follow .agents/skills/verify/SKILL.md steps 1 and 2, then retry." >&2
exit 1
