-- Mangrove LSP setup for Neovim (0.10+, native `vim.lsp`).
--
-- Usage (with the `mangrove` binary on $PATH):
--
--   require("mangrove").setup()
--
-- This registers the `mangrove` filetype (via ftdetect/mangrove.lua), starts
-- `mangrove lsp` for every `.mang` buffer, and enables semantic-token
-- highlighting (Mangrove ships no tree-sitter grammar — highlighting comes from
-- the LSP). The server is read-only and never touches the network.

local M = {}

function M.setup(opts)
  opts = opts or {}
  local cmd = opts.cmd or { "mangrove", "lsp" }

  vim.api.nvim_create_autocmd("FileType", {
    pattern = "mangrove",
    callback = function(args)
      vim.lsp.start({
        name = "mangrove",
        cmd = cmd,
        root_dir = vim.fs.dirname(vim.fs.find(
          { "mangrove.lock", ".git" },
          { upward = true, path = vim.api.nvim_buf_get_name(args.buf) }
        )[1]) or vim.fn.getcwd(),
      })
    end,
  })
end

return M
