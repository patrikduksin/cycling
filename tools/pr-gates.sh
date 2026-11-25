#!/usr/bin/env bash
set -u

words=()
harmless=0
admin=0
while (($#)); do
  case $1 in
    # These take a value, so the word after one is never a flag: `--body -h` is a body of "-h".
    -R | --repo | -a | --assignee | -B | --base | -b | --body | -F | --body-file | -H | --head | -l | --label | \
      -m | --milestone | -p | --project | --recover | -r | --reviewer | -T | --template | -t | --title) shift ;;
    --dry-run | --help | -h) harmless=1 ;;
    --admin) admin=1 ;;
    -*) ;;
    *) words+=("$1") ;;
  esac
  shift
done
# Before the harmless flags: a merge flag the list above misses could take one as its value.
# --admin is Patrik's bypass: a PR he waived the gates for merges without verify or a review.
if [[ ${words[0]-} == pr && ${words[1]-} == merge ]] && ((!admin)); then
  echo "gh pr merge refused: cycling merge <pr> merges; gh pr merge <pr> --admin --squash only for a PR Patrik waived the gates for." >&2
  exit 1
fi
((harmless)) && exit 0
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
echo "gh pr create refused: $gates not run in this session. Follow .agents/skills/verify/SKILL.md step 1, then retry." >&2
exit 1
