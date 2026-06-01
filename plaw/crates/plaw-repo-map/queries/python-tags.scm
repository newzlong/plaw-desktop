; Python — definitions
(class_definition
  name: (identifier) @name.definition.class) @definition.class

(function_definition
  name: (identifier) @name.definition.function) @definition.function

; Module-level constants: NAME = ...
(module
  (expression_statement
    (assignment
      left: (identifier) @name.definition.constant))) @definition.constant

; Python — references
(call
  function: (identifier) @name.reference.call) @reference.call

(call
  function: (attribute
    attribute: (identifier) @name.reference.call)) @reference.call
