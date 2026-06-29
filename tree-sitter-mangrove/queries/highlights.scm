; Keywords
"use" @keyword
"as" @keyword
"type" @keyword
"unit" @keyword
"schema" @keyword
"params" @keyword
"fn" @keyword
"match" @keyword
(unset) @keyword
"brand" @keyword
"require" @keyword
"patch" @keyword
"append" @keyword
"remove" @keyword
"int" @type.builtin
"decimal" @type.builtin
"str" @type.builtin
"bool" @type.builtin
"bytes" @type.builtin

; Declarations
(type_def name: (identifier) @type)
(unit_def name: (identifier) @type)
(schema_decl name: (identifier) @type)
(schema_decl name: (dotted_name) @type)

; Functions
(fn_def name: (identifier) @function)
(fn_call name: (identifier) @function.call)
(module_call alias: (identifier) @function.call)

; Parameters
(param_decl name: (identifier) @variable.parameter)
(fn_param name: (identifier) @variable.parameter)

; Field names in records and types
(binding key: (identifier) @property)
(binding key: (string) @property)
(field_type name: (identifier) @property)
(field_type name: (string) @property)
(unit_member name: (identifier) @constant)
(match_arm pattern: (identifier) @property)
(named_arg name: (identifier) @property)

; Spread
(spread name: (identifier) @variable)
(list_op key: (identifier) @property)

; Type names (references)
(named_type (identifier) @type)
(module_ref alias: (identifier) @namespace)
(module_ref name: (identifier) @type)

; Annotations
(annotation "@" @attribute)
(annotation name: (identifier) @attribute)

; Literals
(string) @string
(raw_string) @string
(text_block) @string
(raw_text_block) @string
(bytes) @string.special
(integer) @number
(decimal) @number.float
(unit_literal) @number.special
(bool) @boolean
(wildcard) @variable.special

; Comments
(comment) @comment
(doc_comment) @comment.documentation
(directive) @preproc

; Operators / punctuation
"=" @operator
"=~" @operator
"&" @operator
"|" @operator
">=" @operator
"<=" @operator
">" @operator
"<" @operator
"==" @operator
"!=" @operator
"+=" @operator
"?" @operator
"*" @operator
"..." @punctuation.special
"." @punctuation.delimiter
"," @punctuation.delimiter
":" @punctuation.delimiter
"@" @punctuation.special
"{" @punctuation.bracket
"}" @punctuation.bracket
"[" @punctuation.bracket
"]" @punctuation.bracket
"(" @punctuation.bracket
")" @punctuation.bracket
