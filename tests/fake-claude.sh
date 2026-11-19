#!/usr/bin/env bash
# Stand-in for `claude -p --input-format stream-json` in tests. Reads one JSON
# line per turn and answers with a result line, without using any tokens.
#   FAKE_STUBBORN=1  never agrees, to test the budgets
#   FAKE_ASK=1       asks a human on its first turn, agrees once a human answered
n=0
human=0
answer() {
  printf '{"type":"result","is_error":false,"result":"%s","usage":{"input_tokens":100,"output_tokens":10}}\n' "$1"
}
while IFS= read -r line; do
  case "$line" in *"· HUMAN]"*) human=1 ;; esac
  case "$line" in
    *"Write the briefing"*) answer "Goal: agree on a plan. Known: nothing yet." ;;
    *"First line: KNOWN_MISSING"*) answer "KNOWN_MISSING\\nI know the goal. I am missing the deadline." ;;
    *"fill the gaps"*) answer "ANSWER\\nThe deadline is Friday." ;;
    *"reached consensus"*) answer "FINAL: the agreed plan." ;;
    *"closes without consensus"*) answer "OVERVIEW: agreed nothing, disputed everything." ;;
    *"Your turn"*)
      n=$((n + 1))
      if [ "${FAKE_STUBBORN:-0}" = 1 ]; then answer "CRITIQUE\\nStill not convinced ($n)."
      elif [ "$human" = 1 ]; then answer "CONSENSUS\\nAgreed with the human's answer."
      elif [ "${FAKE_ASK:-0}" = 1 ] && [ "$n" = 1 ]; then answer "NEED_HUMAN\\nWhich database should we use?"
      elif [ "$n" = 1 ]; then answer "PROPOSAL\\nDo it in two steps."
      else answer "CONSENSUS\\nAgreed."
      fi ;;
    *) answer "PASS" ;;
  esac
done
