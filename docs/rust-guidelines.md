# Rust Guidelines

Write Rust that is simple, explicit, idiomatic, and easy to maintain.

## Priorities

In order of importance:

1. Correctness
2. Simplicity
3. Readability
4. Maintainability
5. Performance, when measured and required

Prefer boring and predictable code over clever code.

## Design

- Implement the simplest solution that satisfies the current requirement.
- Start with concrete structs, enums, and functions.
- Do not introduce traits, generics, factories, plugins, or extra layers for hypothetical future needs.
- Add abstractions only after at least two real use cases demonstrate a common pattern.
- Keep modules and crates few and cohesive.
- Avoid unrelated refactors while implementing a focused change.
- Optimize for local reasoning: a function should be understandable from its signature, nearby types, and visible control flow.

## Domain Modeling

- Use structs to represent domain entities and value groups.
- Use enums for closed sets of states or mutually exclusive alternatives.
- Prefer enums over combinations of boolean flags.
- Use `Option<T>` only when absence is valid and expected.
- Use `Result<T, E>` for recoverable failures.
- Make invalid states unrepresentable when doing so remains simple.
- Introduce newtypes only when they provide validation, semantic distinction, restricted construction, or domain behavior.
- Do not create a separate type for every field by default.

## Ownership and Mutation

- Prefer immutable bindings.
- Add `mut` only when mutation is necessary and keep its scope narrow.
- Prefer constructing complete values over mutating default instances field by field.
- Prefer owned values when they simplify the design.
- Use references for temporary access and ownership when storing or consuming values.
- A small clone is often preferable to complicated lifetimes or shared ownership.
- Do not introduce `Rc`, `Arc`, `RefCell`, `Mutex`, or complex lifetimes before reconsidering the ownership model.

## Control Flow

- Keep the success path visible and close to the left margin.
- Use `?` to propagate errors when the current function cannot handle them meaningfully.
- Use early returns and `let-else` to avoid nesting.
- Use `match` when all domain states require explicit handling.
- Avoid wildcard match arms for internal domain enums when exhaustiveness is useful.
- Use expression-oriented code when it reduces mutation and branching.
- Do not compress logic into a single expression when intermediate variables improve clarity.

## Iteration

- Use iterators when the transformation is immediately understandable.
- Use a straightforward `for` loop when it is clearer than an iterator chain.
- Break long chains into named intermediate values.
- Avoid chains with nested closures, mixed side effects, or difficult error handling.

## Error Handling

- Do not use panics for expected operational failures.
- Avoid `unwrap()` and `expect()` in production paths.
- `expect()` is acceptable only when the invariant is local, obvious, and explained by the message.
- Preserve underlying error information.
- Add context at meaningful boundaries such as filesystem, network, process, parsing, or persistence operations.
- Do not convert errors to `String` too early.
- Keep error types proportional to what callers need to distinguish.

## Functions and Side Effects

- Keep functions focused on one coherent responsibility.
- Extract functions when they name a domain operation, isolate validation, isolate side effects, or remove meaningful duplication.
- Do not extract functions mechanically based on line count.
- Separate pure domain logic from I/O where practical.
- Make side effects obvious in names and signatures.
- Pass dependencies explicitly.
- Avoid hidden global state and service-locator patterns.

## APIs

- Prefer descriptive names over abbreviations.
- Avoid multiple boolean parameters; use enums or option structs.
- Use `Default` only when the default is unsurprising and valid.
- Construct security-sensitive or behavior-defining configuration explicitly.
- Keep public APIs small.
- Prefer private items or `pub(crate)` over unnecessary `pub`.
- Do not create a trait solely to mock one implementation.
- Use dynamic dispatch when it is simpler than propagating generics through the codebase.

## Standard Library and Dependencies

- Check the standard library before implementing common behavior manually.
- Prefer `Path` and `PathBuf` for filesystem paths.
- Prefer `Duration` for time intervals.
- Prefer collection APIs such as `entry`, `is_empty`, iterators, and `Option::take`.
- Add a dependency only when it removes meaningful complexity or correctness risk.
- Avoid dependencies that replace only a few clear lines of standard Rust.
- Do not reimplement security-sensitive or protocol-level functionality merely to avoid a dependency.

## Async and Performance

- Use async only when real concurrent I/O exists.
- Keep domain logic synchronous when possible.
- Do not optimize based on assumptions.
- Prefer simple collections and owned data until measurement proves they are inadequate.
- Before optimizing, define a requirement, measure, identify the bottleneck, and preserve a benchmark or test.
- Do not use `unsafe` without a concrete requirement that safe Rust cannot satisfy.

## Comments and Documentation

- Comments should explain decisions, constraints, invariants, or non-obvious tradeoffs.
- Do not write comments that merely restate the code.
- Document why an unusual design is necessary.
- Keep documentation synchronized with behavior.

## Tests

- Test observable behavior and domain invariants.
- Avoid tests coupled to private helpers or incidental implementation details.
- Prefer pure unit tests for domain logic and integration tests for external boundaries.
- Add regression tests for fixed bugs.
- Keep tests readable and explicit.

## Required Checks

Before finishing a change, run the project-equivalent of:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Do not silence warnings without understanding them. Keep lint allowances narrow and justified.

## Final Review

Before completing code, verify:

- Is this the simplest correct implementation?
- Is every abstraction required by a real use case?
- Could a concrete function or type replace a trait or generic?
- Are domain states explicit and valid?
- Is mutation limited?
- Is the success path easy to follow?
- Are errors handled at the right boundary?
- Is a loop clearer than the iterator chain?
- Are ownership and lifetimes simpler than necessary?
- Is async or optimization actually required?
- Are side effects visible?
- Can the standard library replace custom code?
- Do tests validate behavior rather than implementation?
- Would another Rust developer understand the change without extensive navigation?
