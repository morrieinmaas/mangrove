# tree-sitter-mangrove

A [Tree-sitter](https://tree-sitter.github.io/tree-sitter/) grammar for the
[Mangrove](https://github.com/morrieinmaas/mangrove) configuration language
(`.mang` files).

Covers the full Mangrove syntax: type definitions, unit types, schema
declarations, params blocks, function definitions, bindings, records, lists,
match expressions, string interpolation, refinement types, union types,
annotations, spreads, list-op blocks, and all literal forms (integers,
decimals, unit literals, strings, raw strings, text blocks, bytes, booleans).

## Usage

### Neovim (nvim-treesitter)

1. Make sure you have [nvim-treesitter](https://github.com/nvim-treesitter/nvim-treesitter) installed.

2. Register the grammar in your Neovim config:

```lua
local parser_config = require("nvim-treesitter.parsers").get_parser_configs()
parser_config.mangrove = {
  install_info = {
    url = "/path/to/mangrove/tree-sitter-mangrove",  -- local path
    -- or a remote URL: "https://github.com/yourorg/tree-sitter-mangrove",
    files = { "src/parser.c" },
    branch = "main",
  },
  filetype = "mang",
}
vim.treesitter.language.register("mangrove", "mang")
```

3. Install the parser:

```vim
:TSInstall mangrove
```

4. Optionally, add filetype detection for `.mang` files if not already handled
   by the Mangrove Neovim plugin in `editors/nvim/`:

```lua
vim.filetype.add({ extension = { mang = "mang" } })
```

5. Highlights are provided by `queries/highlights.scm`. Copy or symlink it to
   your nvim-treesitter queries directory:

```sh
mkdir -p ~/.config/nvim/queries/mangrove
cp queries/highlights.scm ~/.config/nvim/queries/mangrove/
```

### Zed

Zed has built-in Tree-sitter support. To use this grammar in Zed:

1. The Mangrove Zed extension in `editors/zed/` already wires up the LSP.
   Tree-sitter highlighting is an optional additional layer.

2. Add a `grammar` entry to the `extension.toml` in `editors/zed/` pointing
   at this grammar directory, or submit the grammar to the
   [Zed extensions repository](https://github.com/zed-industries/extensions).

3. Place `queries/highlights.scm` where the extension can find it (the
   `queries/` directory inside the extension).

## Development

```sh
# Install tree-sitter CLI (if not already installed)
npm install -g tree-sitter-cli

# Regenerate the parser after editing grammar.js
tree-sitter generate

# Run corpus tests
tree-sitter test

# Parse example files
tree-sitter parse ../examples/k8s-deployment.mang
tree-sitter parse ../examples/k8s-templated.mang
tree-sitter parse ../examples/pyproject.mang
```

## File structure

```
tree-sitter-mangrove/
  grammar.js          # Grammar definition
  src/
    parser.c          # Generated parser (committed for consumers)
  queries/
    highlights.scm    # Highlight queries
  test/
    corpus/
      basics.txt      # Corpus tests
```
