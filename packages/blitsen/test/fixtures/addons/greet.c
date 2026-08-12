// The smallest thing that is genuinely a Node-API addon: one registration entry
// point exporting one string. The napi_* symbols are resolved from the hosting
// executable at load time, so this compiles with a bare C compiler and no Node
// headers — `cc -shared -fPIC -o greet.node greet.c`.
#include <stddef.h>

typedef void *napi_env;
typedef void *napi_value;

extern int napi_create_string_utf8(napi_env env, const char *text, size_t length, napi_value *result);
extern int napi_set_named_property(napi_env env, napi_value object, const char *name, napi_value value);

napi_value napi_register_module_v1(napi_env env, napi_value exports) {
  napi_value greeting;
  if (napi_create_string_utf8(env, "blitsen-addon-ok", (size_t)-1, &greeting) != 0) return exports;
  napi_set_named_property(env, exports, "greeting", greeting);
  return exports;
}
