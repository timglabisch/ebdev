# ebdev example

## Usage

```bash
./ebdev tasks              # list available tasks
./ebdev task hello         # run a simple task
./ebdev task greet -- --name World --loud   # run a task with args
```

The `./ebdev` wrapper script builds the binary from the workspace on first run.

## Tasks with arguments

Tasks defined with `defineTask` support typed CLI arguments:

```typescript
export const greet = defineTask({
  description: "Greet someone",
  args: {
    name: arg.string("Name to greet").required(),
    loud: arg.boolean("Shout the greeting"),
  },
  async run({ name, loud }) {
    const msg = `Hello, ${name}!`;
    await exec(["echo", loud ? msg.toUpperCase() : msg]);
  },
});
```

These args are passed after `--` and automatically show up in shell completions.

## Shell Completions (zsh)

```bash
./ebdev completions zsh >> ~/.zshrc
```

Restart your shell, then:

```bash
./ebdev task <TAB>          # completes task names (with descriptions)
./ebdev task greet -- <TAB> # completes --name, --loud
```
