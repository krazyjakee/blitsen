#include <node_api.h>
#include <uv.h>

static napi_value pump_uv(napi_env env, napi_callback_info info) {
  uv_loop_t *loop = NULL;
  napi_status status = napi_get_uv_event_loop(env, &loop);
  if (status != napi_ok || loop == NULL) {
    napi_throw_error(env, NULL, "napi_get_uv_event_loop failed");
    return NULL;
  }

  int result = uv_run(loop, UV_RUN_NOWAIT);
  napi_value output;
  napi_create_int32(env, result, &output);
  return output;
}

NAPI_MODULE_INIT() {
  napi_value function;
  napi_create_function(env, "pumpUv", NAPI_AUTO_LENGTH, pump_uv, NULL, &function);
  napi_set_named_property(env, exports, "pumpUv", function);
  return exports;
}
