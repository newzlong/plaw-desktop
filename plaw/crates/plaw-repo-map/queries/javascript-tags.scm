; JavaScript — definitions
(function_declaration
  name: (identifier) @name.definition.function) @definition.function

(class_declaration
  name: (identifier) @name.definition.class) @definition.class

(method_definition
  name: (property_identifier) @name.definition.method) @definition.method

; const X = ...
(lexical_declaration
  (variable_declarator
    name: (identifier) @name.definition.constant)) @definition.constant

(variable_declaration
  (variable_declarator
    name: (identifier) @name.definition.constant)) @definition.constant

; JavaScript — references
(call_expression
  function: (identifier) @name.reference.call) @reference.call

(call_expression
  function: (member_expression
    property: (property_identifier) @name.reference.call)) @reference.call

(new_expression
  constructor: (identifier) @name.reference.class) @reference.class
