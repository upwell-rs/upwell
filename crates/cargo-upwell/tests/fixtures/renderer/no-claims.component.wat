;; A capability-free implementation of crates/cargo-upwell/wit/renderer.wit.
;; Identical to valid.component.wat except that it returns no resource claims.
(component
  (core module $renderer
    (memory (export "memory") 64)
    (data (i32.const 1024) "customtext/plainresource:onefixture output")
    (global $heap (mut i32) (i32.const 4096))

    (func (export "cabi_realloc")
      (param $old i32) (param $old-len i32) (param $align i32) (param $new-len i32)
      (result i32)
      (local $ptr i32)
      global.get $heap
      local.set $ptr
      global.get $heap
      local.get $new-len
      i32.add
      i32.const 7
      i32.add
      i32.const -8
      i32.and
      global.set $heap
      local.get $ptr)

    ;; render-request lowers to 15 flat i32 fields. The response uses an indirect
    ;; result at 2048: ok(render-response), with an empty resources list.
    (func (export "render")
      (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      (result i32)
      i32.const 2048
      i32.const 0
      i32.store
      i32.const 2048
      i32.const 1024
      i32.store offset=4
      i32.const 2048
      i32.const 6
      i32.store offset=8
      i32.const 2048
      i32.const 1030
      i32.store offset=12
      i32.const 2048
      i32.const 10
      i32.store offset=16
      i32.const 2048
      i32.const 2100
      i32.store offset=20
      i32.const 2048
      i32.const 0
      i32.store offset=24
      i32.const 2100
      i32.const 1040
      i32.store
      i32.const 2100
      i32.const 12
      i32.store offset=4
      i32.const 2048
      i32.const 1052
      i32.store offset=28
      i32.const 2048
      i32.const 14
      i32.store offset=32
      i32.const 2048)

    (func (export "cabi_post_render") (param i32)))

  (core instance $renderer-instance (instantiate $renderer))
  (alias core export $renderer-instance "memory" (core memory $memory))
  (alias core export $renderer-instance "cabi_realloc" (core func $realloc))
  (alias core export $renderer-instance "render" (core func $render))
  (alias core export $renderer-instance "cabi_post_render" (core func $post-render))

  (type $render-request (record
    (field "abi-version" string)
    (field "command" string)
    (field "format" string)
    (field "media-type" string)
    (field "tooling-schema" string)
    (field "resources" (list string))
    (field "color" bool)
    (field "payload" (list u8))))
  (type $render-response (record
    (field "format" string)
    (field "media-type" string)
    (field "resources" (list string))
    (field "body" (list u8))))
  (import "render-request" (type $imported-render-request (eq $render-request)))
  (import "render-response" (type $imported-render-response (eq $render-response)))
  (type $render-result (result $imported-render-response (error string)))
  (type $render-type (func (param "request" $imported-render-request) (result $render-result)))
  (func $render-lifted (type $render-type)
    (canon lift (core func $render)
      (memory $memory)
      (realloc $realloc)
      string-encoding=utf8
      (post-return $post-render)))
  (export "render" (func $render-lifted)))
