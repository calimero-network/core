# Contribution Guide

[GitPR]: https://github.com/susam/gitpr
[Issues]: https://github.com/calimero-network/core/issues
[README]: README.mdx

Thank you for dedicating your time to contribute to our project.

This guide outlines the contribution workflow to make the process smooth and
effective for everyone.

Start by reading the [README][] to understand the project better.

## Project Status

This project is actively being developed.

You can check out the open [Issues][], monitor the development progress, and
contribute.

## Getting Started

There are several ways you can contribute:

- Solve open [Issues][]
- Report bugs or suggest features
- Enhance the documentation

Contributions are managed via Issues and Pull Requests (PRs). Here are some
general guidelines:

- Read the Rust style guide bellow!

- Before creating a new Issue or PR, search for [existing ones][Issues].

- Contributions should focus on either functionality or style in the PR, not
  both.

- If you encounter an error, provide context. Explain what you are trying to do
  and how to reproduce the error.

- Follow the repository’s formatting guidelines.

- Update the [README][] file if your changes affect it.

## Issues

Use [Issues][] to report problems, request features, or discuss changes before
creating a PR.

### Solving an Issue

Browse [existing issues][Issues] to find one that interests you.

## Contribution Guidelines for Working on Issues

If someone is already working on an issue, they will either be officially
assigned to it or have left a comment indicating they are working on it. If you
would like to work on an issue, please follow these steps:

1. **Comment on the Issue**: Leave a comment on the issue expressing your
   intention to work on it. For example, "I would like to work on this issue."

2. **Wait for Confirmation**: A project maintainer will confirm your assignment
   by officially assigning the issue to you or by acknowledging your comment.

3. **Start Working**: Once you have received confirmation, you can start working
   on the issue.

4. **Open a Pull Request**: When your work is ready, open a pull request (PR)
   with your solution. Make sure to mention in the PR that you are working on
   the issue by referencing the issue number in the PR description (e.g., "This
   PR addresses issue #123").

By following this process, we can avoid duplication of efforts and ensure clear
communication among all contributors.

## Rust Style Guide

### Formatting

- Use rustfmt with nightly features to maintain consistent code formatting.
- Sort Cargo.toml dependencies alphabetically.

### Module Organization

- We don't use the `mod.rs` pattern.
- Instead, export modules from files named according to their context.

For example:

```bash
crates/meroctl/src/cli/app.rs
```

Contains:

```rust
mod get;
mod install;
mod list;
```

And we would have these individual files:

```bash
crates/meroctl/src/cli/app/get.rs
crates/meroctl/src/cli/app/install.rs
crates/meroctl/src/cli/app/list.rs
```

### Error Handling

- Almost no unwrapping (acceptable in tests and possibly when dealing with thread join handlers).
- If unwrapping is absolutely necessary, explain why with a comment.
- On values that may return errors, use `.map_err()` to map the error into the appropriate Error type used in that crate/module.

### Code Efficiency

- Try to limit unnecessary clones.
- Use short-circuiting if statements:

```rust
// NOT RECOMMENDED:
if some_condition {
    // ... (lots of code)
}

// RECOMMENDED:
if !some_condition {
    return Err(YourError::Something);
}
// Continue with main code path...
```

- Extract values from options using `if let` when possible:

```rust
// RECOMMENDED:
if let Some(value) = optional_value {
    // Use value directly
}

// INSTEAD OF:
match optional_value {
    Some(value) => {
        // Use value
    },
    None => {},
}
```

### Code Organization

- Break functions into smaller parts if they become too large.
- Place reusable functions in a `commons.rs` file or similar.
- Put structs needed by multiple parts of the codebase into `primitives` files.

### Naming Conventions

- Types shall be `UpperCamelCase`
- Enum variants shall be `UpperCamelCase`
- Struct fields shall be `snake_case`
- Function and method names shall be `snake_case`
- Local variables shall be `snake_case`
- Macro names shall be `snake_case`
- Constants (consts and immutable statics) shall be `SCREAMING_SNAKE_CASE`

### General Guidelines

- Try to use functional Rust patterns when they improve code readability and maintainability.

## Creating a New Issue

If no related issue exists, you can create a new one.

Here are some tips:

- Provide detailed context to make it clear for others.
- Include steps to reproduce the issue or the rationale for a new feature.
- Attach screenshots, videos, etc., if applicable.

## Pull Requests

### Pull Request Workflow

We use the ["fork-and-pull"][GitPR] Git workflow:

1. Fork the repository.

2. Clone the project.

3. Create a new branch with a descriptive name.

4. Commit your changes to this new branch.

5. Push your changes to your fork.

6. Create a pull request from your fork to our repository. Use the `master`
   branch as the base branch.

7. Tag a maintainer to review your PR.

8. Make sure your PR follows our PR template (has to consist of a `Description`, `Test plan` and `Documentation update`)

### Tips for a Quality Pull Request

- Title your PR to clearly describe the work done.

- Structure your description based on our PR template

- Link to the related issue, if applicable.

- Write a concise commit message summarizing the work.

### After Submitting Your PR

- We might ask questions, request more details, or ask for changes before
  merging your PR. This ensures clarity and smooth interaction.

- As you update your PR, resolve each conversation.

- Once approved, we will "squash-and-merge" to keep the commit history clean.

Thank you for your contributions!
