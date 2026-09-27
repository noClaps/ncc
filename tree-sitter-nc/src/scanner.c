#include "tree_sitter/parser.h"

enum { CALL_OPEN, INDEX_OPEN, STRUCT_OPEN, VALUE_START, ERROR_SENTINEL };

void *tree_sitter_nc_external_scanner_create(void) { return NULL; }
void tree_sitter_nc_external_scanner_destroy(void *payload) { (void)payload; }
unsigned tree_sitter_nc_external_scanner_serialize(void *payload, char *buffer) {
  (void)payload;
  (void)buffer;
  return 0;
}
void tree_sitter_nc_external_scanner_deserialize(void *payload, const char *buffer, unsigned length) {
  (void)payload;
  (void)buffer;
  (void)length;
}
bool tree_sitter_nc_external_scanner_scan(void *payload, TSLexer *lexer, const bool *valid) {
  (void)payload;
  if (valid[ERROR_SENTINEL]) return false;
  while (lexer->lookahead == ' ' || lexer->lookahead == '\t') lexer->advance(lexer, true);
  lexer->mark_end(lexer);
  if (lexer->eof(lexer) || lexer->lookahead == '\n' || lexer->lookahead == '\r') return false;
  // NC comments extend to the end of the line, so cannot bridge a postfix call.
  if (lexer->lookahead == '/') {
    lexer->advance(lexer, false);
    if (lexer->lookahead == '/') return false;
  }
  if (valid[VALUE_START] && lexer->lookahead != '}') {
    lexer->result_symbol = VALUE_START;
    return true;
  }
  const char openings[] = {'(', '[', '{'};
  for (unsigned token = CALL_OPEN; token <= STRUCT_OPEN; token++) {
    if (valid[token] && lexer->lookahead == openings[token]) {
      lexer->advance(lexer, false);
      lexer->mark_end(lexer);
      if (token == STRUCT_OPEN) {
        for (;;) {
          while (lexer->lookahead == ' ' || lexer->lookahead == '\t' || lexer->lookahead == '\r' || lexer->lookahead == '\n') lexer->advance(lexer, false);
          if (lexer->lookahead != '/') break;
          lexer->advance(lexer, false);
          if (lexer->lookahead != '/') return false;
          while (!lexer->eof(lexer) && lexer->lookahead != '\n') lexer->advance(lexer, false);
        }
        if (lexer->lookahead != '.') return false;
      }
      lexer->result_symbol = token;
      return true;
    }
  }
  return false;
}
