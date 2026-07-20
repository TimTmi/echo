# Coding Rules: Echo

## Priority Order
1. Correctness
2. Simplicity
3. Readability
4. Maintainability
5. Performance
6. Cleverness

---

## Coding Standards

### General
- Prefer simple solutions over abstract/flexible designs
- Keep files focused on one responsibility
- Delete dead code immediately
- Avoid premature optimization
- Avoid unnecessary abstractions

### Structure
- Standard Rust project layout: `src/main.rs`, `src/lib.rs`, modules under `src/`
- Organize modules by feature (e.g., `src/qdrant/`, `src/embedding/`, `src/tui/`, `src/config/`)
- Use `cargo clippy` and `cargo fmt` before committing
- Follow Rust API Guidelines for public interfaces

### Complexity
- Max function length: ~40 lines unless justified
- Max nesting depth: 3
- Prefer early returns over nested conditionals
- Refactor duplicated logic immediately after second occurrence

### Naming
- Use full descriptive names
- Avoid generic names like `util`, `helper`, `manager`, `misc`
- Boolean names should read naturally (e.g., `isFull`, `hasConflict`)

### State and Side Effects
- Prefer pure functions where possible
- Avoid hidden side effects
- Pass dependencies explicitly
- No mutable global state

### Error Handling
- Never swallow exceptions silently
- Add contextual information when rethrowing errors
- Return user-safe error messages at API boundaries
- Use `anyhow` for application-level error handling, `thiserror` for library-style errors

### Testing
- Critical business logic must be unit-testable without TUI framework
- Use `cargo test` — prefer unit tests in-module, integration tests in `tests/`
- Mock external services (Qdrant, llama.cpp) with mock HTTP servers for integration tests

### Git Workflow
- Make small, focused commits with descriptive messages
- Commit after each meaningful piece of work completes (new feature, fix, refactor, context update)
- Push after every commit (or batch of closely related commits) to keep remote current
- Keep commits granular enough that rolling one back is safe

### Refactor Triggers
Refactor immediately when:
- duplicate logic appears twice
- function needs comment to explain flow
- parameter count exceeds 4
- branching becomes difficult to follow
- file handles multiple domains/responsibilities

### Forbidden
- giant service classes
- god objects
- boolean parameter traps
- deep inheritance trees
- copy-paste reuse
- static mutable globals (use `OnceCell` or `LazyLock` if unavoidable)

---

## Documentation
- Update `decisions.md` whenever significant architectural decisions change
- Keep `progress.md` updated after major milestones
- Documentation must reflect current implementation reality, not plans
- Record important tradeoffs and rejected approaches in `decisions.md`

---

## Interaction
- When service/domain complexity grows, update `design.md` or add a focused `.context` file before implementation
- Ask for clarification instead of guessing critical business rules
- Prefer incremental implementation over massive one-shot generation
- If existing code violates rules, refactor nearby code while working
