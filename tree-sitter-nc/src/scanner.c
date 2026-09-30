#include "tree_sitter/parser.h"

#include <stddef.h>

// Keep this order in sync with grammar.js externals.
enum TokenType {
  SAME_LINE,
  CALL_LPAREN,
  INDEX_LBRACKET,
  STRUCT_LBRACE,
  ERROR_SENTINEL,
};

void *tree_sitter_nc_external_scanner_create(void) { return NULL; }

void tree_sitter_nc_external_scanner_destroy(void *payload) { (void)payload; }

unsigned tree_sitter_nc_external_scanner_serialize(void *payload,
                                                   char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}

void tree_sitter_nc_external_scanner_deserialize(void *payload,
                                                 const char *buffer,
                                                 unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}

bool tree_sitter_nc_external_scanner_scan(void *payload, TSLexer *lexer,
                                          const bool *valid_symbols) {
  (void)payload;
  // Error recovery enables every external token. Do not fabricate a zero-width
  // value marker or steal punctuation from recovery's ordinary lexer.
  if (valid_symbols[ERROR_SENTINEL]) {
    return false;
  }

  while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
         lexer->lookahead == '\r' || lexer->lookahead == '\f' ||
         lexer->lookahead == '\v') {
    lexer->advance(lexer, true);
  }
  if (lexer->lookahead == '\n' || lexer->eof(lexer)) {
    return false;
  }

  if (valid_symbols[SAME_LINE]) {
    if (lexer->lookahead == '}' || lexer->lookahead == ':') {
      return false;
    }
    // Comments are extras, but their line end must still terminate an optional
    // return/break value. Failed lookahead is rewound by Tree-sitter.
    lexer->mark_end(lexer);
    if (lexer->lookahead == '/') {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '/') {
        return false;
      }
    }
    lexer->result_symbol = SAME_LINE;
    return true;
  }

  enum TokenType symbol;
  switch (lexer->lookahead) {
  case '(':
    symbol = CALL_LPAREN;
    break;
  case '[':
    symbol = INDEX_LBRACKET;
    break;
  case '{':
    symbol = STRUCT_LBRACE;
    break;
  default:
    return false;
  }
  if (!valid_symbols[symbol]) {
    return false;
  }
  lexer->advance(lexer, false);
  lexer->mark_end(lexer);
  lexer->result_symbol = symbol;
  return true;
}
