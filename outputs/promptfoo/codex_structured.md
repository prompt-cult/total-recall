

## Accomplished
- Created a single-file Snake game script at `/tmp/snake.py` using Python with a `#!/usr/bin/env -S uv run` shebang and PEP 723 metadata for dependencies (pygame).
- Implemented classic snake mechanics: grid-based movement, food collection, collision detection, score tracking, and game over/restart functionality.
- Spawned a subagent ("Hubble") to replicate the snake game's structure for a new project.
- Successfully generated a Space Invaders game at `/tmp/space_invaders.py` following the exact same format and structure as the snake game.

## Current Work
- Both game scripts are complete and ready for execution. No active development is currently in progress.

## Files Involved
- `/tmp/snake.py` (Snake game implementation)
- `/tmp/space_invaders.py` (Space Invaders implementation)

## Next Steps
- Execute either game using `./tmp/snake.py` or `./tmp/space_invaders.py`.
- Alternatively, run via `uv run /tmp/snake.py` or `uv run /tmp/space_invaders.py`.

## Key Decisions/Constraints
- **Format:** Both scripts must be single-file executables with `uv` shebang and PEP 723 metadata.
- **Dependencies:** `pygame` is specified in script metadata for automatic installation.
- **Structure:** Games use a main class (`SpaceInvadersGame` for invaders) with methods: `__init__`, `reset_game`, `handle_events`, `update`, `draw`, `run`.
- **Controls:** Arrow keys for movement, spacebar for firing (invaders) or restarting.
- **FPS:** Snake runs at 10 FPS; Space Invaders runs at 60 FPS.
- **State Management:** Space Invaders uses a `GameState` enum for running/game-over states.