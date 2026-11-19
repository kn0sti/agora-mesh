#!/usr/bin/env bash
# Stand-in for `claude -p --input-format stream-json` in tests. Reads one JSON
# line per turn and answers with a result line, without using any tokens.
# FAKE_STUBBORN=1 makes it never agree, to test the turn budget.
n=0
answer() {
  printf '{"type":"result","is_error":false,"result":"%s","usage":{"input_tokens":100,"output_tokens":10}}\n' "$1"
}
while IFS= read -r line; do
  case "$line" in
    *"Write the briefing"*) answer "Goal: agree on a plan. Known: nothing yet." ;;
    *"First line: KNOWN_MISSING"*) answer "KNOWN_MISSING\\nI know the goal. I am missing the deadline." ;;
    *"Fill the gaps"*) answer "ANSWER\\nThe deadline is Friday." ;;
    *"reached consensus"*) answer "FINAL: the agreed plan." ;;
    *"closes without consensus"*) answer "OVERVIEW: agreed nothing, disputed everything." ;;
    *"Your turn"*)
      n=$((n + 1))
      if [ "${FAKE_STUBBORN:-0}" = 1 ]; then answer "CRITIQUE\\nStill not convinced ($n)."
      elif [ "$n" = 1 ]; then answer "PROPOSAL\\nDo it in two steps."
      else answer "CONSENSUS\\nAgreed."
      fi ;;
    *) answer "PASS" ;;
  esac
done
