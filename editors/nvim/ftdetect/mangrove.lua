-- Detect `.mang` files as the `mangrove` filetype.
vim.filetype.add({
  extension = {
    mang = "mangrove",
  },
})
