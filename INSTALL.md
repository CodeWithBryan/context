# Installing ctx

`ctx` is a local-first MCP context engine for TypeScript/JS/CSS/HTML codebases.
Binaries are code-signed and notarized on macOS — no Gatekeeper workaround needed.

---

## Quick install (macOS)

```sh
# Apple Silicon (M1/M2/M3/M4):
curl -L https://github.com/CodeWithBryan/context/releases/latest/download/ctx-latest-aarch64-apple-darwin.tar.gz | tar xz

# Intel Mac:
curl -L https://github.com/CodeWithBryan/context/releases/latest/download/ctx-latest-x86_64-apple-darwin.tar.gz | tar xz

# Move to somewhere on your PATH:
mv ctx /usr/local/bin/ctx
# — or —
mkdir -p ~/.local/bin && mv ctx ~/.local/bin/
```

Binaries are signed with a Developer ID Application certificate and notarized by
Apple, so macOS will run them without any `xattr` workaround.

---

## Quick install (Linux x86_64)

```sh
curl -L https://github.com/CodeWithBryan/context/releases/latest/download/ctx-latest-x86_64-unknown-linux-gnu.tar.gz | tar xz
mv ctx /usr/local/bin/ctx
```

---

## Verify

```sh
ctx --version
```

---

## First run

```sh
cd my-project
ctx init .
ctx index .      # first time: ~150 MB model download + ~7 min indexing
ctx serve .      # starts MCP stdio server
```

> **TypeScript symbol extraction** requires `tsgo` to be installed in your
> project. Without it semantic search still works, but `find_definition` only
> returns CSS/HTML symbols.
>
> ```sh
> bun add -d @typescript/native-preview
> # or
> npm install -D @typescript/native-preview
> ```

---

## Updating

```sh
ctx update           # prompts for confirmation
ctx update --force   # skip the prompt
```

---

## MCP client configuration

### Claude Code

Add the following to your Claude Code MCP settings (`.claude/settings.json` or
global `~/.claude/settings.json`):

```json
{
  "mcpServers": {
    "ctx": {
      "command": "ctx",
      "args": ["serve", "."],
      "type": "stdio"
    }
  }
}
```

### Claude Desktop

In `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "ctx": {
      "command": "/usr/local/bin/ctx",
      "args": ["serve", "/absolute/path/to/your/project"]
    }
  }
}
```

Replace `/absolute/path/to/your/project` with the path to the repository you
want to index. Run `ctx init .` and `ctx index .` inside that project first.
