# Writers run one at a time in v1

All Writers in a Session share one worktree, so they run sequentially. Running them in parallel would make it unclear which Work Unit broke the build. Parallel Writers, each in its own worktree with merging afterwards, are a known TODO, not an oversight.
