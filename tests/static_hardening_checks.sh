#!/usr/bin/env bash
set -euo pipefail

fail() {
  echo "static hardening check failed: $*" >&2
  exit 1
}

grep -q -- "-lcurl" build.sh || fail "build.sh should link curl portably"
! grep -q "/lib/x86_64-linux-gnu/libcurl.so.4" build.sh || \
  fail "build.sh must not hard-code a distro-specific curl path"

test ! -e src/safety.cpp || fail "unused safety.cpp should stay removed"
test ! -e src/safety.h || fail "unused safety.h should stay removed"

grep -q "chmod(sock_path.c_str(), 0600)" src/daemon.cpp || \
  fail "daemon socket should be private"
grep -q "MAX_CLIENT_THREADS" src/daemon.cpp || \
  fail "daemon should cap concurrent client handlers"

grep -q "const symptomDesc = .*json(symptom_desc).dump()" src/causal_engine.cpp || \
  fail "HTML report symptom string should be JSON encoded"
grep -q "escapeHtml" src/causal_engine.cpp || \
  fail "HTML report should escape dynamic inspector data"

! grep -q 'key=' src/ai.cpp src/codebase.cpp || \
  fail "API keys must not be embedded into curl URLs"
grep -q "x-goog-api-key" src/ai.cpp || \
  fail "Gemini API key should be sent through private curl config headers"

grep -q "\\[K\\] SIGKILL" src/ui/tui.cpp || \
  fail "TUI should document uppercase K for SIGKILL"
