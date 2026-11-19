#!/bin/bash
# Fixture advisor for crates/bran-cli/tests/brief_select.rs.
# BRAN clears the environment, so every variable read here must be named in pass_env.
# ADVISOR_MARKER: write this process ID first, to show a spawn happened.
# ADVISOR_STDIN: copy the request read on stdin.
# ADVISOR_ENV_OUT: write the names (never the values) of the received environment.
# ADVISOR_REPLY: file whose content is printed on stdout.
# ADVISOR_MODE: empty (print the reply, exit 0), sleep, exit (print, exit 3) or big (reply plus 1 MiB of spaces).
if [ -n "$ADVISOR_MARKER" ]; then printf '%s\n' "$$" >"$ADVISOR_MARKER"; fi
request=$(</dev/stdin)
if [ -n "$ADVISOR_STDIN" ]; then printf '%s' "$request" >"$ADVISOR_STDIN"; fi
if [ -n "$ADVISOR_ENV_OUT" ]; then
	while IFS= read -r -d '' pair; do printf '%s\n' "${pair%%=*}"; done </proc/$$/environ >"$ADVISOR_ENV_OUT"
fi
reply=""
if [ -n "$ADVISOR_REPLY" ]; then reply=$(<"$ADVISOR_REPLY"); fi
if [ "$ADVISOR_MODE" = sleep ]; then
	exec /bin/sleep 30
elif [ "$ADVISOR_MODE" = exit ]; then
	printf '%s' "$reply"
	exit 3
elif [ "$ADVISOR_MODE" = big ]; then
	printf '%s' "$reply"
	pad=$(printf '%1024s' '')
	for ((n = 0; n < 1024; n++)); do printf '%s' "$pad"; done
	exit 0
fi
printf '%s' "$reply"
