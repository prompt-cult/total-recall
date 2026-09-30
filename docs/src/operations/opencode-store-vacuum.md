{{#include ../../opencode-store-vacuum.md}}

## Source and script

This page reproduces
[`docs/opencode-store-vacuum.md`](https://github.com/prompt-cult/total-recall/blob/main/docs/opencode-store-vacuum.md),
which is the canonical copy in the repository. The procedure is implemented by
[`scripts/opencode-db-vacuum.sh`](../scripts/opencode-db-vacuum.sh), available here
for download, with subcommands `check`, `counts`, `backup`, `vacuum-into`,
`prune-events`, `swap`, `verify`. Its store path comes from the `OPENCODE_DB`
environment variable, defaulting to
`$HOME/.local/share/opencode/opencode.db`.
