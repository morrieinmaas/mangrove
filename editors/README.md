# Editor integration

The Mangrove language server (`mangrove lsp`) is a read-only, network-free LSP
over stdio. It provides diagnostics (parse + schema errors), hover, document
symbols, semantic-token highlighting, and formatting (via `mangrove fmt`).

> Highlighting note: Mangrove ships **no tree-sitter grammar** — syntax
> highlighting comes from the LSP's semantic tokens. A `.mang` file has no
> highlighting until the server attaches.

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
(`K`), document symbols, and `vim.lsp.buf.format()` work out of the box;
semantic-token highlighting is enabled automatically.

Custom binary path:

```lua
require("mangrove").setup({ cmd = { "/abs/path/to/mangrove", "lsp" } })
```

## Other editors

Any LSP client can launch `mangrove lsp` over stdio. Point your client's
language-server command at `mangrove lsp` and associate it with the `.mang`
extension. A Zed extension is a planned follow-up.
