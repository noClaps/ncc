-- Neovim 0.11+, using built-in LSP and Tree-sitter APIs only.
local M = {}

function M.setup(root)
  root = vim.fs.normalize(root)
  vim.filetype.add({ extension = { nc = 'nc' } })
  vim.treesitter.language.add('nc', { path = root .. '/target/nc.so' })
  local query = table.concat(vim.fn.readfile(root .. '/tree-sitter-nc/queries/highlights.scm'), '\n')
  vim.treesitter.query.set('nc', 'highlights', query)
  vim.lsp.config('ncc', {
    cmd = { root .. '/target/release/ncc', 'lsp' },
    filetypes = { 'nc' },
    root_markers = { '.git' },
  })
  vim.lsp.enable('ncc')
  vim.api.nvim_create_autocmd('FileType', {
    group = vim.api.nvim_create_augroup('ncc', { clear = true }),
    pattern = 'nc',
    callback = function(event)
      vim.treesitter.start(event.buf, 'nc')
      vim.bo[event.buf].commentstring = '// %s'
      vim.bo[event.buf].shiftwidth = 2
      vim.bo[event.buf].expandtab = true
    end,
  })
end

return M
