#!/usr/bin/env sh
set -eu

export DD_SERVICE="${DD_SERVICE:-temperpaw}"
export DD_ENV="${DD_ENV:-prod}"
export DD_VERSION="${DD_VERSION:-${BUILD_SHA:-unknown}}"
export OTEL_RESOURCE_ATTRIBUTES="${OTEL_RESOURCE_ATTRIBUTES:-service.name=${DD_SERVICE},service.version=${DD_VERSION},deployment.environment=${DD_ENV},dd_llmobs_enabled=false}"

# The host lookup cache (/etc/nscd.conf), before the server, so the server's lookups find it. nscd returns once its
# socket is ready and then runs in the background. /var/run/nscd may be missing (nothing creates it without systemd),
# and a pid file left by an earlier run of this container can name a live process, which would stop nscd. The server
# starts either way: without nscd its lookups go to the resolver, as they did before.
start_nscd() {
  mkdir -p /var/run/nscd && rm -f /var/run/nscd/nscd.pid && /usr/sbin/nscd
}
if ! start_nscd; then
  echo "nscd did not start; host lookups go to the resolver directly" >&2
fi

if [ "${TEMPER_DDPROF_ENABLED:-false}" = "true" ]; then
  if command -v ddprof >/dev/null 2>&1; then
    exec ddprof \
      --service "${DD_SERVICE}" \
      --environment "${DD_ENV}" \
      --service_version "${DD_VERSION}" \
      ./temperpaw
  fi
  echo "TEMPER_DDPROF_ENABLED=true but ddprof is not installed; starting without native profiling" >&2
fi

exec ./temperpaw
