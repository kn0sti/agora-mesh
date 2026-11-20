#!/usr/bin/env bash
# Stand-in for `claude -p --input-format stream-json` in tests. Reads one JSON
# line per turn and answers with a result line, without using any tokens.
#   FAKE_STUBBORN=1  never agrees, to test the budgets
#   FAKE_ASK=1       asks a human on its first turn, agrees once a human answered
#   FAKE_SPLIT=1     proposes a split of the work on its first turn
#   FAKE_SPLIT=2     the same, with two packages for the opener
#   FAKE_PASS=1      has nothing to add on its first turn, agrees afterwards
#   FAKE_LIMIT=1     fails with a usage-limit error when asked to work on a package
#   FAKE_UTIL        utilization it reports with every turn (default 0.42)
#   FAKE_DELAY       seconds to wait before every answer
# The arguments of every start are appended to $AGORA_HOME/fake-args.log.
# A start that is pointed at another model service is noted in
# $AGORA_HOME/fake-env.log: the address, whether the key is the one in
# FAKE_EXPECT_TOKEN (the key itself is not written), and the Anthropic key it sees.
[ -n "${AGORA_HOME:-}" ] && echo "$*" >> "$AGORA_HOME/fake-args.log"
if [ -n "${AGORA_HOME:-}" ] && [ -n "${ANTHROPIC_BASE_URL:-}" ]; then
  ok=no; [ "${ANTHROPIC_AUTH_TOKEN:-}" = "${FAKE_EXPECT_TOKEN:-}" ] && ok=yes
  echo "base=$ANTHROPIC_BASE_URL token_ok=$ok api_key=[${ANTHROPIC_API_KEY-unset}] model=${ANTHROPIC_MODEL:-}" >> "$AGORA_HOME/fake-env.log"
fi
n=0
human=0
memory=""
answer() {
  [ -n "${FAKE_DELAY:-}" ] && sleep "$FAKE_DELAY"
  printf '{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":4102444800,"unifiedWindows":{"five_hour":{"utilization":%s,"resetsAt":4102444800,"status":"allowed"}}}}\n' "${FAKE_UTIL:-0.42}"
  printf '{"type":"result","is_error":false,"result":"%s","usage":{"input_tokens":100,"output_tokens":10}}\n' "$1"
}
limit() {
  printf '{"type":"result","is_error":true,"api_error_status":429,"result":"You have hit your usage limit","usage":{"input_tokens":0,"output_tokens":0}}\n'
}
while IFS= read -r line; do
  case "$line" in *"· HUMAN]"*) human=1 ;; esac
  # Says so once it has been told about a new shared-memory entry.
  case "$line" in *"[shared memory, new]"*) memory=" saw-memory-notice" ;; esac
  case "$line" in
    *"Review this change"*)
      case "$line" in
        *"+changed line"*) answer "notes.txt:1 - saw the diff - looks fine" ;;
        *) answer "no diff arrived" ;;
      esac ;;
    *"connection test"*) answer "ANSWER\\napple" ;;
    *"Which word did you reply with"*) answer "ANSWER\\napple" ;;
    *"Reply with only the word: "*) answer "one" ;;
    *"hands you a task"*) answer "DONE\\nDid the task. cwd=$(basename "$PWD")" ;;
    *"asks you one question"*) answer "The deadline is Friday. cwd=$(basename "$PWD")" ;;
    *"The work phase is over"*) answer "WORKRESULT: all packages are reported." ;;
    *"Work phase. Package #"*)
      if [ "${FAKE_LIMIT:-0}" = 1 ]; then limit
      else
        plan=""
        case "$line" in *"The room agreed on this plan"*) plan=" got-the-plan" ;; esac
        answer "DONE\\nDid the work. cwd=$(basename "$PWD")$plan"
      fi ;;
    *"Write the briefing"*) answer "Goal: agree on a plan. Known: nothing yet." ;;
    *"First line: KNOWN_MISSING"*) answer "KNOWN_MISSING\\nI know the goal. I am missing the deadline. cwd=$(basename "$PWD")" ;;
    *"fill the gaps"*) answer "ANSWER\\nThe deadline is Friday." ;;
    *"reached consensus"*) answer "FINAL: the agreed plan." ;;
    *"closes without consensus"*) answer "OVERVIEW: agreed nothing, disputed everything." ;;
    *"You join this room while it is running"*)
      n=$((n + 1))
      case "$line" in
        *"· BRIEF]"*) answer "CRITIQUE\\nLate, but I read what was said. saw-catch-up" ;;
        *) answer "CRITIQUE\\nLate, and nobody told me anything." ;;
      esac ;;
    *"Your turn"*)
      n=$((n + 1))
      if [ "${FAKE_STUBBORN:-0}" = 1 ]; then answer "CRITIQUE\\nStill not convinced ($n).$memory"
      elif [ "$human" = 1 ]; then answer "CONSENSUS\\nAgreed with the human's answer."
      elif [ "${FAKE_PASS:-0}" = 1 ] && [ "$n" = 1 ]; then answer "PASS\\nNothing to add yet."
      elif [ "${FAKE_PASS:-0}" = 1 ]; then answer "CONSENSUS\\nAgreed."
      elif [ "${FAKE_ASK:-0}" = 1 ] && [ "$n" = 1 ]; then answer "NEED_HUMAN\\nWhich database should we use?"
      elif [ "${FAKE_SPLIT:-0}" = 1 ] && [ "$n" = 1 ]; then answer "SPLIT_WORK\\n- Mac: backend\\n- Heimserver: frontend"
      elif [ "${FAKE_SPLIT:-0}" = 2 ] && [ "$n" = 1 ]; then answer "SPLIT_WORK\\n- Mac: backend\\n- Mac: docs\\n- Heimserver: frontend"
      elif [ "$n" = 1 ]; then answer "PROPOSAL\\nDo it in two steps."
      else answer "CONSENSUS\\nAgreed."
      fi ;;
    *) answer "PASS" ;;
  esac
done
