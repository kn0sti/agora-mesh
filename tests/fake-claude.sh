#!/usr/bin/env bash
# Stand-in for `claude -p --input-format stream-json` in tests. Reads one JSON
# line per turn and answers with a result line, without using any tokens.
#   FAKE_STUBBORN=1  never agrees, to test the budgets
#   FAKE_ASK=1       asks a human on its first turn, agrees once a human answered
#   FAKE_SPLIT=1     proposes a split of the work on its first turn
#   FAKE_LIMIT=1     fails with a usage-limit error when asked to work on a package
#   FAKE_UTIL        utilization it reports with every turn (default 0.42)
n=0
human=0
answer() {
  printf '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":4102444800,"unifiedWindows":{"five_hour":{"utilization":%s,"resetsAt":4102444800,"status":"allowed"}}}}\n' "${FAKE_UTIL:-0.42}"
  printf '{"type":"result","is_error":false,"result":"%s","usage":{"input_tokens":100,"output_tokens":10}}\n' "$1"
}
limit() {
  printf '{"type":"result","is_error":true,"api_error_status":429,"result":"You have hit your usage limit","usage":{"input_tokens":0,"output_tokens":0}}\n'
}
while IFS= read -r line; do
  case "$line" in *"· HUMAN]"*) human=1 ;; esac
  case "$line" in
    *"The work phase is over"*) answer "WORKRESULT: all packages are reported." ;;
    *"Work phase. Package #"*)
      if [ "${FAKE_LIMIT:-0}" = 1 ]; then limit
      else answer "DONE\\nDid the work. cwd=$(basename "$PWD")"
      fi ;;
    *"Write the briefing"*) answer "Goal: agree on a plan. Known: nothing yet." ;;
    *"First line: KNOWN_MISSING"*) answer "KNOWN_MISSING\\nI know the goal. I am missing the deadline. cwd=$(basename "$PWD")" ;;
    *"fill the gaps"*) answer "ANSWER\\nThe deadline is Friday." ;;
    *"reached consensus"*) answer "FINAL: the agreed plan." ;;
    *"closes without consensus"*) answer "OVERVIEW: agreed nothing, disputed everything." ;;
    *"Your turn"*)
      n=$((n + 1))
      if [ "${FAKE_STUBBORN:-0}" = 1 ]; then answer "CRITIQUE\\nStill not convinced ($n)."
      elif [ "$human" = 1 ]; then answer "CONSENSUS\\nAgreed with the human's answer."
      elif [ "${FAKE_ASK:-0}" = 1 ] && [ "$n" = 1 ]; then answer "NEED_HUMAN\\nWhich database should we use?"
      elif [ "${FAKE_SPLIT:-0}" = 1 ] && [ "$n" = 1 ]; then answer "SPLIT_WORK\\n- Mac: backend\\n- Heimserver: frontend"
      elif [ "$n" = 1 ]; then answer "PROPOSAL\\nDo it in two steps."
      else answer "CONSENSUS\\nAgreed."
      fi ;;
    *) answer "PASS" ;;
  esac
done
