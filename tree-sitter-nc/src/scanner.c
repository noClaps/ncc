#include "tree_sitter/parser.h"

#include <stddef.h>
#include <string.h>

// Keep this order in sync with grammar.js externals.
enum TokenType {
  NEWLINE,
  OP_OR,
  OP_AND,
  OP_EQ,
  OP_NE,
  OP_LT,
  OP_LE,
  OP_GT,
  OP_GE,
  OP_IN,
  OP_BIT_OR,
  OP_XOR,
  OP_BIT_AND,
  OP_SHL,
  OP_SHR,
  OP_PLUS,
  OP_MINUS,
  OP_CONCAT,
  OP_MUL,
  OP_DIV,
  OP_MOD,
  OP_POW,
  OP_MEMBER,
  OP_ASSIGN,
  OP_ELSE,
  OP_CATCH,
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

static bool whitespace(int32_t c) {
  return c == ' ' || c == '\t' || c == '\r' || c == '\n' || c == '\f' ||
         c == '\v';
}

static bool word_character(int32_t c) {
  return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') ||
         (c >= '0' && c <= '9') || c == '_';
}

// Read the complete operator, never splitting a longer operator or keyword.
static int continuation(TSLexer *lexer, const bool *valid_symbols) {
  static const char *const operators[] = {
      "or", "and", "==", "!=", "<",  "<=",   ">",     ">=", "in",
      "|",  "^",   "&",  "<<", ">>", "+",    "-",     "<>", "*",
      "/",  "%",   "**", ".",  "=",  "else", "catch",
  };
  char text[6] = {0};
  size_t length = 0;
  int32_t first = lexer->lookahead;
  if (word_character(first)) {
    while (word_character(lexer->lookahead)) {
      if (length == sizeof(text) - 1) {
        return -1;
      }
      text[length++] = (char)lexer->lookahead;
      lexer->advance(lexer, false);
    }
  } else if (first == '=' || first == '!' || first == '<' || first == '>' ||
             first == '*' || first == '/' || first == '%' || first == '+' ||
             first == '-' || first == '|' || first == '&' || first == '^' ||
             first == '.') {
    text[length++] = (char)first;
    lexer->advance(lexer, false);
    if (lexer->lookahead == '=' ||
        (first == '<' &&
         (lexer->lookahead == '<' || lexer->lookahead == '>')) ||
        (first == '>' && lexer->lookahead == '>') ||
        (first == '*' && lexer->lookahead == '*') ||
        (first == '-' && lexer->lookahead == '>') ||
        (first == '/' && lexer->lookahead == '/')) {
      text[length++] = (char)lexer->lookahead;
      lexer->advance(lexer, false);
    }
  }
  for (size_t i = 0; i < sizeof(operators) / sizeof(operators[0]); i++) {
    int symbol = OP_OR + (int)i;
    if (valid_symbols[symbol] && strcmp(text, operators[i]) == 0) {
      return symbol;
    }
  }
  return -1;
}

// Comments must remain named extras rather than become part of an operator.
// Probe beyond them only to decide whether the newline is a separator.
static bool comment_continuation(TSLexer *lexer, const bool *valid_symbols) {
  while (lexer->lookahead == '/') {
    lexer->advance(lexer, true);
    if (lexer->lookahead != '/') {
      return valid_symbols[OP_DIV];
    }
    while (lexer->lookahead != '\n' && !lexer->eof(lexer)) {
      lexer->advance(lexer, true);
    }
    while (whitespace(lexer->lookahead)) {
      lexer->advance(lexer, true);
    }
  }
  return continuation(lexer, valid_symbols) >= 0;
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

  // Probe separators before the existing postfix/type logic. Failed scans
  // rewind lookahead, so ordinary extras still retain comments and expression
  // whitespace.
  if (valid_symbols[NEWLINE]) {
    bool newline = false;
    while (whitespace(lexer->lookahead)) {
      bool line_end = lexer->lookahead == '\n';
      // Keep continuation-token ranges on the operator, not its leading line
      // break. A separator still advances through the marked first newline.
      lexer->advance(lexer, true);
      if (line_end && !newline) {
        lexer->mark_end(lexer);
      }
      newline |= line_end;
    }
    if (newline) {
      if (lexer->lookahead == '/') {
        if (comment_continuation(lexer, valid_symbols)) {
          return false;
        }
      } else {
        int symbol = continuation(lexer, valid_symbols);
        if (symbol >= 0) {
          // Preserve type-application alternatives while a name expression is
          // also live; the grammar shares angle tokens with comparisons.
          if (symbol == OP_LT && valid_symbols[TYPE_LT]) {
            symbol = TYPE_LT;
          } else if (symbol == OP_GT && valid_symbols[TYPE_GT]) {
            symbol = TYPE_GT;
          }
          lexer->mark_end(lexer);
          lexer->result_symbol = (TSSymbol)symbol;
          return true;
        }
      }
      lexer->result_symbol = NEWLINE;
      return true;
    }
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
    if (lexer->lookahead == '}' || lexer->lookahead == ':' ||
        lexer->lookahead == ';') {
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
