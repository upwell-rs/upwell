;; An ABI-compatible renderer whose render function never returns. The fixture
;; proves the host's epoch deadline independently of fuel exhaustion.
(component
  (core module $renderer
    (memory (export "memory") 64)
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

    (func (export "render")
      (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32 i32)
      (result i32)
      (loop $forever
        br $forever)
      unreachable)

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
