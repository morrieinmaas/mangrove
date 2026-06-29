/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

module.exports = grammar({
  name: 'mangrove',

  extras: $ => [
    /\s/,
    $.comment,
    $.doc_comment,
    $.directive,
  ],

  word: $ => $.identifier,

  conflicts: $ => [
    // fn_call vs module_call (both are `ident(...)`)
    [$.fn_call, $.module_call],
  ],

  rules: {
    source_file: $ => repeat($._statement),

    // ---- Statements ----

    _statement: $ => choice(
      $.use_decl,
      $.type_def,
      $.unit_def,
      $.schema_decl,
      $.params_block,
      $.fn_def,
      $.spread,
      $.list_op,
      $.binding,
    ),

    use_decl: $ => seq(
      'use',
      field('path', $.string),
      'as',
      field('alias', $.identifier),
    ),

    type_def: $ => seq(
      'type',
      field('name', $.identifier),
      '=',
      field('type', $._type_expr),
      repeat($.annotation),
    ),

    unit_def: $ => seq(
      'unit',
      field('name', $.identifier),
      ':',
      field('base', $._primitive_type),
      '{',
      field('members', commaSep1($.unit_member)),
      optional(','),
      '}',
    ),

    unit_member: $ => seq(
      field('name', $.identifier),
      '=',
      field('value', $._unit_value),
    ),

    _unit_value: $ => choice(
      $.unit_literal,
      $.integer,
      $.decimal,
    ),

    schema_decl: $ => seq(
      'schema',
      field('name', $._schema_ref),
      optional(seq('&', field('extra', $.record_type))),
    ),

    _schema_ref: $ => choice(
      $.dotted_name,
      $.identifier,
    ),

    dotted_name: $ => seq(
      $.identifier,
      repeat1(seq('.', $.identifier)),
    ),

    params_block: $ => seq(
      'params',
      '{',
      repeat($.param_decl),
      '}',
    ),

    param_decl: $ => seq(
      field('name', $._key),
      ':',
      field('type', $._type_expr),
      optional(seq('=', field('default', $._value))),
      optional(','),
    ),

    fn_def: $ => seq(
      'fn',
      field('name', $.identifier),
      '(',
      commaSep($.fn_param),
      ')',
      ':',
      field('return_type', $._type_expr),
      '=',
      field('body', $._value),
    ),

    fn_param: $ => seq(
      field('name', $.identifier),
      ':',
      field('type', $._type_expr),
    ),

    binding: $ => seq(
      field('key', $._key),
      ':',
      field('value', $._value),
    ),

    spread: $ => seq(
      '...',
      field('name', $.identifier),
    ),

    list_op: $ => seq(
      field('key', $._key),
      '+=',
      '[',
      repeat(seq($._value, optional(','))),
      ']',
    ),

    // ---- Keys ----

    _key: $ => choice(
      $.identifier,
      $.string,
    ),

    // ---- Values ----

    _value: $ => choice(
      $.record,
      $.list,
      $.match_expr,
      $.fn_call,
      $.module_call,
      $.unit_literal,
      $.integer,
      $.decimal,
      $.string,
      $.raw_string,
      $.text_block,
      $.raw_text_block,
      $.bytes,
      $.bool,
      $.unset,
      $.identifier,
    ),

    record: $ => seq(
      '{',
      repeat(seq($._record_entry, optional(','))),
      '}',
    ),

    _record_entry: $ => choice(
      $.patch_op,
      $.list_op,
      $.spread,
      $.binding,
    ),

    patch_op: $ => seq(
      field('key', $._key),
      '{',
      repeat(seq($._patch_stmt, optional(','))),
      '}',
    ),

    _patch_stmt: $ => choice(
      $.patch_item,
      $.append_item,
      $.remove_item,
    ),

    patch_item: $ => seq('patch', field('key', $.string), ':', field('value', $._value)),
    append_item: $ => seq('append', ':', field('value', $._value)),
    remove_item: $ => seq('remove', ':', field('key', $.string)),

    list: $ => seq(
      '[',
      repeat(seq($._value, optional(','))),
      ']',
    ),

    match_expr: $ => seq(
      'match',
      field('subject', $.identifier),
      '{',
      repeat(seq($.match_arm, optional(','))),
      '}',
    ),

    match_arm: $ => seq(
      field('pattern', $._match_pattern),
      ':',
      field('value', $._value),
    ),

    _match_pattern: $ => choice(
      $.wildcard,
      $.identifier,
      $.string,
      $.integer,
      $.bool,
    ),

    wildcard: $ => '_',

    fn_call: $ => seq(
      field('name', $.identifier),
      '(',
      commaSep($._value),
      ')',
    ),

    module_call: $ => seq(
      field('alias', $.identifier),
      '(',
      commaSep($.named_arg),
      ')',
    ),

    named_arg: $ => seq(
      field('name', $.identifier),
      ':',
      field('value', $._value),
    ),

    // ---- Type expressions ----

    _type_expr: $ => choice(
      $.union_type,
      $._type_atom,
    ),

    union_type: $ => prec.left(1, seq(
      $._type_atom,
      repeat1(seq('|', $._type_atom)),
    )),

    _type_atom: $ => choice(
      $.refined_type,
      $._base_type,
    ),

    refined_type: $ => prec.left(2, seq(
      $._base_type,
      repeat1(seq('&', $._refinement)),
    )),

    _refinement: $ => choice(
      $.comparison_refinement,
      $.regex_refinement,
      $.record_type,
      $.brand_type,
    ),

    comparison_refinement: $ => seq(
      field('op', choice('>=', '<=', '>', '<', '==', '!=')),
      field('value', choice($.integer, $.decimal, $.string, $.bool)),
    ),

    regex_refinement: $ => seq(
      '=~',
      field('pattern', choice($.string, $.raw_string)),
    ),

    brand_type: $ => seq('brand', $._base_type),

    _base_type: $ => choice(
      $._primitive_type,
      $.named_type,
      $.list_type,
      $.record_type,
      $.literal_type,
      seq('(', $._type_expr, ')'),
    ),

    _primitive_type: $ => choice(
      'int',
      'decimal',
      'str',
      'bool',
      'bytes',
    ),

    named_type: $ => choice(
      $.module_ref,
      $.identifier,
    ),

    module_ref: $ => seq(
      field('alias', $.identifier),
      '.',
      field('name', $.identifier),
    ),

    list_type: $ => seq(
      '[',
      field('element', $._type_expr),
      ']',
    ),

    record_type: $ => seq(
      '{',
      repeat(seq($._record_type_entry, optional(','))),
      '}',
    ),

    _record_type_entry: $ => choice(
      $.field_type,
      $.map_type_entry,
      $.require_clause,
    ),

    field_type: $ => prec.right(0, seq(
      field('name', $._key),
      optional('?'),
      ':',
      field('type', $._type_expr_with_default),
      field('annotations', repeat($.annotation)),
    )),

    _type_expr_with_default: $ => choice(
      $.default_type,
      $._type_expr,
    ),

    default_type: $ => prec.right(0, seq(
      $._type_expr,
      '|',
      '*',
      field('default', $._value),
    )),

    map_type_entry: $ => seq(
      '[',
      field('key_type', $._primitive_type),
      ']',
      ':',
      field('value_type', $._type_expr),
    ),

    require_clause: $ => seq(
      'require',
      ':',
      field('pred', $._value),
      repeat($.annotation),
    ),

    literal_type: $ => choice(
      $.string,
      $.raw_string,
      $.integer,
      $.decimal,
      $.bool,
    ),

    // ---- Annotations ----

    annotation: $ => seq(
      '@',
      field('name', $.identifier),
      optional(seq(
        '(',
        commaSep($._annotation_arg),
        ')',
      )),
    ),

    _annotation_arg: $ => choice(
      $.string,
      $.identifier,
    ),

    // ---- Terminals ----

    // unit_literal has higher precedence so `512Mi` is lexed as one token
    unit_literal: $ => token(prec(2, seq(
      /[0-9][0-9_]*/,
      optional(seq('.', /[0-9][0-9_]*/)),
      /[a-zA-Z_][a-zA-Z0-9_]*/,
    ))),

    integer: $ => token(prec(1, /[0-9][0-9_]*/)),

    decimal: $ => token(prec(1, seq(
      /[0-9][0-9_]*/,
      '.',
      /[0-9][0-9_]*/,
    ))),

    // String: any content including ${...} interpolations, treated as opaque.
    string: $ => token(seq(
      '"',
      repeat(choice(
        /[^"\\]/,
        /\\./,
      )),
      '"',
    )),

    raw_string: $ => token(seq('r"', /[^"]*/, '"')),

    // Text blocks: triple-quoted. Content is any chars (including newlines).
    // Use a character-class approach that works with tree-sitter's RE2 engine.
    text_block: $ => token(seq(
      '"""',
      /[^"]*/,
      '"""',
    )),

    raw_text_block: $ => token(seq(
      'r"""',
      /[^"]*/,
      '"""',
    )),

    bytes: $ => token(seq('b64"', /[A-Za-z0-9+/=]*/, '"')),

    bool: $ => choice('true', 'false'),

    unset: $ => 'unset',

    identifier: $ => /[a-zA-Z_][a-zA-Z0-9_-]*/,

    // Comments: `##` is doc_comment, `#!` is directive, `#` is regular comment.
    // Order matters: longer prefix wins due to prec values.
    doc_comment: $ => token(prec(2, /##[^\n]*/)),
    directive: $ => token(prec(2, /#![^\n]*/)),
    comment: $ => token(prec(1, /#[^\n]*/)),
  },
});

function commaSep(rule) {
  return optional(seq(rule, repeat(seq(',', rule)), optional(',')));
}

function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)));
}
