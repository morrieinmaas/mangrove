# Editor integration

The Mangrove language server (`mangrove lsp`) is a read-only, network-free LSP
over stdio. It provides diagnostics (parse + schema errors), hover, document
symbols, semantic-token highlighting, context-aware completion, go-to-definition
(local and cross-file into imported types), find-references, rename, and formatting (via `mangrove fmt`).

> Highlighting note: Mangrove ships a **tree-sitter grammar** in
> `tree-sitter-mangrove/` for editors that support Tree-sitter (Neovim,
> Zed, Helix, etc.). The grammar provides immediate syntax highlighting
> on file open, before the LSP attaches. For full semantic highlighting
> (types, references, schema errors), the LSP semantic tokens take over
> once the server connects.

## Neovim (0.10+)

`editors/nvim/` contains a filetype-detection file and a small setup module.

1. Put the `mangrove` binary on your `$PATH` (`cargo install --path crates/mangrove-cli`).
2. Make the plugin visible to Neovim, e.g. add `editors/nvim/` to your
   `runtimepath`, or symlink its contents into your config:

   ```lua
   -- in your init.lua
   vim.opt.runtimepath:append("/path/to/mangrove/editors/nvim")
   require("mangrove").setup()
   ```

That starts `mangrove lsp` for every `.mang` buffer. Diagnostics, hover
(`K`), document symbols, go-to-definition, find-references, rename, completion, and `vim.lsp.buf.format()` work out of the box;
semantic-token highlighting is enabled automatically.

Custom binary path:

```lua
require("mangrove").setup({ cmd = { "/abs/path/to/mangrove", "lsp" } })
```

## Zed

`editors/zed/` is a Zed dev extension. It registers the `Mangrove` language
(`.mang` files) and wires up `mangrove lsp` as the language server.

### Prerequisites

Put the `mangrove` binary on your `$PATH`:

```sh
cargo install --path crates/mangrove-cli
```

### Install as a dev extension

1. Open Zed.
2. Open the Extensions panel (`Cmd+Shift+X` / `Ctrl+Shift+X`).
3. Click **Install Dev Extension** and select the `editors/zed/` directory.
4. Zed compiles the extension (wasm32 build) and activates it automatically.

### Confirming it works

Open any `.mang` file. You should see:

- Diagnostics (red squiggles) for parse/schema errors.
- Hover (`Cmd+K Cmd+I`) showing type information.
- Completions as you type.
- Syntax highlighting via the Tree-sitter grammar in `tree-sitter-mangrove/`
  (available immediately on open) plus semantic-token highlighting from
  the LSP once it attaches.
- Go-to-definition (`F12` / `Cmd+Click`) for local and imported symbols.
- Document outline in the Outline panel.

### Troubleshooting

If the language server fails to start, check that `mangrove` is on `$PATH`:

```sh
which mangrove
mangrove --version   # prints the version (note: `mangrove lsp` runs the server on stdio and waits for a client)
```

Zed's log panel (`View > Toggle Log`) shows LSP stderr output.

## Other editors

Any LSP client can launch `mangrove lsp` over stdio. Point your client's
language-server command at `mangrove lsp` and associate it with the `.mang`
extension.
