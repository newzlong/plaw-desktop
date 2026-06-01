; Rust — definitions
(struct_item name: (type_identifier) @name.definition.class) @definition.class

(enum_item name: (type_identifier) @name.definition.class) @definition.class

(union_item name: (type_identifier) @name.definition.class) @definition.class

(trait_item name: (type_identifier) @name.definition.interface) @definition.interface

(type_item name: (type_identifier) @name.definition.class) @definition.class

(function_item name: (identifier) @name.definition.function) @definition.function

(function_signature_item name: (identifier) @name.definition.function) @definition.function

(macro_definition name: (identifier) @name.definition.macro) @definition.macro

(mod_item name: (identifier) @name.definition.module) @definition.module

; Rust — references
(call_expression
  function: (identifier) @name.reference.call) @reference.call

(call_expression
  function: (field_expression
    field: (field_identifier) @name.reference.call)) @reference.call

(call_expression
  function: (scoped_identifier
    name: (identifier) @name.reference.call)) @reference.call

(macro_invocation
  macro: (identifier) @name.reference.call) @reference.call

(type_identifier) @name.reference.class @reference.class
