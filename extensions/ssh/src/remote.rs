use crate::command::quote;

pub(super) const SUPERVISOR: &str = r#"
umask 077
directory=$(mktemp -d "${TMPDIR:-/tmp}/pi-ssh.XXXXXXXX") || exit 1
task=
watcher=
stop_tree() {
    for descendant in $(ps -e -o pid= -o ppid= | awk -v parent="$1" '$2 == parent {print $1}'); do
        stop_tree "$descendant"
    done
    kill -KILL "$1" 2>/dev/null || :
}
cleanup() {
    trap '' HUP INT TERM
    [ -z "$watcher" ] || stop_tree "$watcher"
    [ -z "$task" ] || stop_tree "$task"
    rm -rf "$directory"
}
trap 'cleanup 2>/dev/null' EXIT
trap 'exit 130' HUP INT TERM
IFS= read -r size || exit 1
case "$size" in ''|*[!0-9]*) exit 1 ;; esac
dd bs=1 count="$size" of="$directory/input" 2>/dev/null || exit 1
[ "$(wc -c < "$directory/input" | tr -d ' ')" = "$size" ] || exit 1
sh -c "$1" < "$directory/input" &
task=$!
supervisor=$$
exec 3<&0
(cat <&3 >/dev/null; kill -TERM "$supervisor" 2>/dev/null) &
watcher=$!
wait "$task"
code=$?
task=
exit "$code"
"#;

pub fn supervised(command: &str) -> String {
    format!("sh -c {} sh {}", quote(SUPERVISOR), quote(command))
}

pub fn heartbeat_pipe(command: &str) -> String {
    format!("(printf '0\\n'; while sleep 1; do printf '.' || break; done) | ( {command} )")
}
