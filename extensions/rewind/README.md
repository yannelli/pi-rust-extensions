# pi-ext-rewind

Created 2026-10-29. Last updated 2026-10-31.

Per-turn git checkpoints of the workspace, and `/rewind` to restore code, conversation, or both. Port of `@ayulab/pi-rewind` 0.4.6 on the pi-rust native extension SDK. Run it as `pi-rust-ext rewind`.

## How it works

- At the first assistant message of a turn, the extension snapshots the work tree (`beforeCommit`). At `agent_end` it snapshots again (`afterCommit`) and appends a `pi-checkpoint` session entry: `{ v: 2, kind: "checkpoint", turnId, userEntryId, beforeCommit, afterCommit, prompt, fileCount, fileChanges: [{ path, added, removed }], createdAt }`. `createdAt` is milliseconds since the epoch.
- Snapshots go to a bare shadow repo, `<agentDir>/ext/rewind/sessions/<session file stem>/.git` (`ephemeral-<session id>` without a session file). Git runs with `--git-dir` set to the shadow repo and `--work-tree` set to the session cwd. `GIT_DIR`, `GIT_INDEX_FILE`, and related variables are removed from the environment. The user's `.git`, refs, and index stay unchanged.
- An unchanged work tree reuses the previous commit. Every turn still gets an entry, so conversation restore works.
- Staging runs in batches of up to 500 files or 16 MiB; a file of 16 MiB or more gets a batch of its own. Each batch updates the shadow index, so its work is kept when a turn stops waiting.
- A turn waits up to `stageBudgetSeconds` for its pre-turn snapshot. When the work tree is large (for example a home directory as cwd), the first snapshot can take longer. The turn then runs without a checkpoint, the extension shows one notice per session, and staging continues after each turn until the snapshot is complete. A snapshot commits only a complete tree, so every checkpoint restores consistently. When a turn's post-turn snapshot is still unfinished as the next turn starts, the extension drops that turn's checkpoint with one warning, so the checkpoint never takes in later edits. Restores and fork copies stage the whole tree first and do not use the budget.
- A file that git cannot store within `gitTimeoutSeconds` is left out of later snapshots of the session (listed in `<storage>/.git/info/pi-rewind-skip`), with one warning that names it.
- The shadow repo honors the work tree's `.gitignore` files, the built-in excludes, and the `exclude` and `include` settings. Nested git repos and the storage dir are excluded. Files over `maxFileMB` are not snapshotted.
- Snapshots, restores, work-tree comparisons, and fork copies share `<storage>/.pi-checkpoint-lock/`. The lock uses numbered files created exclusively and records the holder's PID and timestamp. It expires after 3 minutes without refresh; other processes wait up to 30 seconds before reporting the holder's PID.
- A restore first takes a safety snapshot, then runs `git reset --hard <commit>` in the shadow repo. Files that are ignored or not snapshotted stay unchanged. When the reset fails, the extension restores the safety snapshot.

## Commands

- `/rewind`: lists the checkpoints of the current branch as `[n] prompt · N files changed +A -R`, plus `(current)`. Then asks for a mode:
  - `Restore code and conversation`
  - `Restore conversation`
  - `Restore code`

  When no turn from the selected one onward changed files and the work tree already matches, only `Restore conversation` shows. Code restore writes the turn's `beforeCommit`. Conversation restore calls `ctx.navigateTree(userEntryId, { summarize: false })` through `pi.call` and puts the prompt back in the editor. A failed file restore does not stop the conversation restore. It shows the `pi-rewind-code-restore-warning` widget above the editor until the next input.

## Events

- `session_start`: loads config. `resume` restores the branch's latest `afterCommit` when `restoreOnResume` is true. `fork` copies the parent session's shadow repo (`git clone --bare --local`). A fork before a user message (`/fork`) restores that turn's `beforeCommit` when `restoreOnFork` is true. A fork at an entry (clone) restores the new branch's latest `afterCommit` when `restoreOnClone` is true.
- `session_before_fork`: writes `fork-intent.json` in the parent storage for the next `session_start`.
- `session_before_tree` and `session_tree`: `/tree` navigation without a summary syncs files per `restoreOnTree`. A user message target restores its `beforeCommit`; another target restores the latest `afterCommit` on the new branch.
- `message_start`, `agent_end`, `session_shutdown`: capture checkpoints. `session_compact`: re-reads the latest `afterCommit` from the branch as the diff base. `input`: clears the warning widget.

## Settings

Same keys as upstream. Layers, later wins per key: pi settings `ayu` (`settings.get`), `<agentDir>/ext/rewind.json`, `<cwd>/.pi/ext/rewind.json`. The two files use the shape of the `ayu` tree: `{ "rewind": {...}, "checkpoint": {...} }`. A malformed file shows a warning and is skipped.

| Key | Default | Use |
| --- | --- | --- |
| `rewind.restoreOnTree` | `"ask"` | `"ask"`, `"never"`, or `"always"`: sync files on `/tree` navigation. |
| `checkpoint.enabled` | `true` | `false` turns off checkpoints and restores. |
| `checkpoint.autoCheckpoint` | `true` | `false` turns off per-turn checkpoints. |
| `checkpoint.restoreOnResume` | `false` | Restore files when a session is resumed. |
| `checkpoint.restoreOnFork` | `false` | Restore files on fork. |
| `checkpoint.restoreOnClone` | `false` | Restore files on clone. |
| `checkpoint.exclude` | `[]` | More exclude patterns (gitignore syntax), after the built-in list. |
| `checkpoint.include` | `[]` | Patterns to keep even when excluded (written as `!pattern`). |
| `checkpoint.maxFileMB` | none | Skip files larger than this. |
| `checkpoint.stageBudgetSeconds` | `10` | How long a turn waits for its pre-turn snapshot. |
| `checkpoint.gitTimeoutSeconds` | `120` | How long git may take to store one batch of files. |

Values of `stageBudgetSeconds` and `gitTimeoutSeconds` that are not positive numbers, or too large for a duration, use the default.

When `ctx.isProjectTrusted` is false, the extension ignores `restoreOnResume`, `restoreOnFork`, `restoreOnClone`, and `restoreOnTree: "always"` from the merged pi settings and `<cwd>/.pi/ext/rewind.json`, and shows a warning. The same keys from `<agentDir>/settings.json` `ayu` and `<agentDir>/ext/rewind.json` still apply.

## Not ported

- The `/checkpoint` storage manager overlay.

## Tests

`cargo test -p pi-ext-rewind`: unit tests (config merge, trust filter, checkpoint parsing, numstat, nested repos), shadow git tests on temp repos (the user's HEAD, index, status, and refs stay the same), budgeted staging (`tests/budget.rs`) and the git timeout skip (`tests/slow_file.rs`), protocol tests against an in-memory host (`/rewind` and `/tree` sync), and an e2e test under the dev pi binary (`PI_BIN`).
