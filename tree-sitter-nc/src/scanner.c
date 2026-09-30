#include "tree_sitter/parser.h"

#include <stddef.h>

// Keep this order in sync with grammar.js externals.
enum TokenType {
  SAME_LINE,
  CALL_LPAREN,
  INDEX_LBRACKET,
  STRUCT_LBRACE,
  TYPE_LT,
  TYPE_GT,
  COMPARISON_GT,
  METADATA_BRACES,
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

// Metadata strings retain brace text without parsing it as an expression. The
// compiler lexer balances braces and respects quoted substrings even here.
static bool metadata_braces(TSLexer *lexer) {
  if (lexer->lookahead != '{') {
    return false;
  }
  size_t depth = 0;
  int32_t quote = 0;
  bool escaped = false;
  do {
    int32_t c = lexer->lookahead;
    lexer->advance(lexer, false);
    if (escaped) {
      escaped = false;
    } else if (quote) {
      if (c == '\\') {
        escaped = true;
      } else if (c == quote) {
        quote = 0;
      }
    } else if (c == '"' || c == '\'') {
      quote = c;
    } else if (c == '{') {
      depth++;
    } else if (c == '}') {
      depth--;
    }
  } while (depth && !lexer->eof(lexer));
  if (depth) {
    return false;
  }
  lexer->mark_end(lexer);
  lexer->result_symbol = METADATA_BRACES;
  return true;
}

bool tree_sitter_nc_external_scanner_scan(void *payload, TSLexer *lexer,
                                          const bool *valid_symbols) {
  (void)payload;
  // Error recovery enables every external token. Do not fabricate a zero-width
  // value marker or steal punctuation from recovery's ordinary lexer.
  if (valid_symbols[ERROR_SENTINEL]) {
    return false;
  }

  if (valid_symbols[METADATA_BRACES]) {
    return metadata_braces(lexer);
  }

  // Type angles may cross lines, but cannot split the lexer's <>, <=, << or >=
  // operators. Nested >> closers are split deliberately by the compiler parser.
  if (valid_symbols[TYPE_LT] || valid_symbols[TYPE_GT] ||
      valid_symbols[COMPARISON_GT]) {
    bool crossed_newline = false;
    while (lexer->lookahead == ' ' || lexer->lookahead == '\t' ||
           lexer->lookahead == '\r' || lexer->lookahead == '\n' ||
           lexer->lookahead == '\f' || lexer->lookahead == '\v') {
      crossed_newline |= lexer->lookahead == '\n';
      lexer->advance(lexer, true);
    }
    int32_t angle = lexer->lookahead;
    if ((angle == '<' && valid_symbols[TYPE_LT]) ||
        (angle == '>' &&
         (valid_symbols[TYPE_GT] || valid_symbols[COMPARISON_GT]))) {
      lexer->advance(lexer, false);
      if (lexer->lookahead == '=' ||
          (angle == '<' &&
           (lexer->lookahead == '>' || lexer->lookahead == '<')) ||
          (angle == '>' && !valid_symbols[TYPE_GT] &&
           lexer->lookahead == '>')) {
        return false;
      }
      lexer->mark_end(lexer);
      lexer->result_symbol = angle == '<'             ? TYPE_LT
                             : valid_symbols[TYPE_GT] ? TYPE_GT
                                                      : COMPARISON_GT;
      return true;
    }
    // Failed scanning rewinds whitespace before the ordinary lexer runs. Do not
    // let type lookahead erase a newline for an optional value/postfix marker.
    if (crossed_newline) {
      return false;
    }
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
