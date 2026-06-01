; TypeScript — definitions (extends JS with type aliases + interfaces)
(function_declaration
  name: (identifier) @name.definition.function) @definition.function

(function_signature
  name: (identifier) @name.definition.function) @definition.function

(class_declaration
  name: (type_identifier) @name.definition.class) @definition.class

(interface_declaration
  name: (type_identifier) @name.definition.interface) @definition.interface

(type_alias_declaration
  name: (type_identifier) @name.definition.class) @definition.class

(enum_declaration
  name: (identifier) @name.definition.class) @definition.class

(method_definition
  name: (property_identifier) @name.definition.method) @definition.method

(method_signature
  name: (property_identifier) @name.definition.method) @definition.method

(abstract_method_signature
  name: (property_identifier) @name.definition.method) @definition.method

(lexical_declaration
  (variable_declarator
    name: (identifier) @name.definition.constant)) @definition.constant

; TypeScript — references
(call_expression
  function: (identifier) @name.reference.call) @reference.call

(call_expression
  function: (member_expression
    property: (property_identifier) @name.reference.call)) @reference.call

(new_expression
  constructor: (identifier) @name.reference.class) @reference.class

(type_identifier) @name.reference.class @reference.class
