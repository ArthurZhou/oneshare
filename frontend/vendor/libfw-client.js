var LibfwClient = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // node_modules/.pnpm/libfw-client@0.1.3/node_modules/libfw-client/index.js
  var libfw_client_exports = {};
  __export(libfw_client_exports, {
    LibfwClient: () => LibfwClient2,
    LibfwError: () => LibfwError,
    default: () => libfw_client_default
  });

  // node_modules/.pnpm/libfw-client@0.1.3/node_modules/libfw-client/pkg/libfw_client.js
  var import_meta = {};
  var LibfwClient = class {
    __destroy_into_raw() {
      const ptr = this.__wbg_ptr;
      this.__wbg_ptr = 0;
      LibfwClientFinalization.unregister(this);
      return ptr;
    }
    free() {
      const ptr = this.__destroy_into_raw();
      wasm.__wbg_libfwclient_free(ptr, 0);
    }
    /**
     * Cancel the active transfer (state → `failed`).
     */
    cancel() {
      wasm.libfwclient_cancel(this.__wbg_ptr);
    }
    /**
     * Bytes transferred so far.
     * @returns {number}
     */
    done_bytes() {
      const ret = wasm.libfwclient_done_bytes(this.__wbg_ptr);
      return ret;
    }
    /**
     * Download a single file at `file_path` into the chosen local directory.
     *
     * Resolves with the number of bytes written.
     * @param {string} base_url
     * @param {string} token
     * @param {string} file_path
     * @returns {Promise<any>}
     */
    download_file(base_url, token, file_path) {
      const ptr0 = passStringToWasm0(base_url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len0 = WASM_VECTOR_LEN;
      const ptr1 = passStringToWasm0(token, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len1 = WASM_VECTOR_LEN;
      const ptr2 = passStringToWasm0(file_path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len2 = WASM_VECTOR_LEN;
      const ret = wasm.libfwclient_download_file(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
      return ret;
    }
    /**
     * Download every file under the virtual `dirPath` (empty = root).
     *
     * Resolves with the number of bytes written.
     * @param {string} base_url
     * @param {string} token
     * @param {string} dir_path
     * @returns {Promise<any>}
     */
    download_folder(base_url, token, dir_path) {
      const ptr0 = passStringToWasm0(base_url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len0 = WASM_VECTOR_LEN;
      const ptr1 = passStringToWasm0(token, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len1 = WASM_VECTOR_LEN;
      const ptr2 = passStringToWasm0(dir_path, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len2 = WASM_VECTOR_LEN;
      const ret = wasm.libfwclient_download_folder(this.__wbg_ptr, ptr0, len0, ptr1, len1, ptr2, len2);
      return ret;
    }
    /**
     * Whether callbacks have been installed.
     * @returns {boolean}
     */
    has_callbacks() {
      const ret = wasm.libfwclient_has_callbacks(this.__wbg_ptr);
      return ret !== 0;
    }
    /**
     * Create an engine. `options` may include:
     * `{ concurrency, compress, chunkSize, maxRetries, baseRetryDelayMs,
     * maxRetryDelayMs, timeoutMs }`.
     * @param {any} opts
     */
    constructor(opts) {
      const ret = wasm.libfwclient_new(opts);
      this.__wbg_ptr = ret;
      LibfwClientFinalization.register(this, this.__wbg_ptr, this);
      return this;
    }
    /**
     * Pause the active transfer (state → `paused`).
     */
    pause() {
      wasm.libfwclient_pause(this.__wbg_ptr);
    }
    /**
     * Progress in `[0, 1]`.
     * @returns {number}
     */
    progress() {
      const ret = wasm.libfwclient_progress(this.__wbg_ptr);
      return ret;
    }
    /**
     * Resume a paused transfer.
     */
    resume() {
      wasm.libfwclient_resume(this.__wbg_ptr);
    }
    /**
     * Install the JS callbacks object (required before any transfer).
     * @param {any} callbacks
     */
    set_callbacks(callbacks) {
      wasm.libfwclient_set_callbacks(this.__wbg_ptr, callbacks);
    }
    /**
     * Current state: `idle | downloading | uploading | paused | completed |
     * failed`.
     * @returns {string}
     */
    state() {
      let deferred1_0;
      let deferred1_1;
      try {
        const ret = wasm.libfwclient_state(this.__wbg_ptr);
        deferred1_0 = ret[0];
        deferred1_1 = ret[1];
        return getStringFromWasm0(ret[0], ret[1]);
      } finally {
        wasm.__wbindgen_free(deferred1_0, deferred1_1, 1);
      }
    }
    /**
     * Total bytes to transfer.
     * @returns {number}
     */
    total_bytes() {
      const ret = wasm.libfwclient_total_bytes(this.__wbg_ptr);
      return ret;
    }
    /**
     * Upload the files reported by the JS `getFileList` callback.
     *
     * Resolves with the number of bytes uploaded.
     * @param {string} base_url
     * @param {string} token
     * @returns {Promise<any>}
     */
    upload(base_url, token) {
      const ptr0 = passStringToWasm0(base_url, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len0 = WASM_VECTOR_LEN;
      const ptr1 = passStringToWasm0(token, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
      const len1 = WASM_VECTOR_LEN;
      const ret = wasm.libfwclient_upload(this.__wbg_ptr, ptr0, len0, ptr1, len1);
      return ret;
    }
  };
  if (Symbol.dispose) LibfwClient.prototype[Symbol.dispose] = LibfwClient.prototype.free;
  function __wbg_get_imports() {
    const import0 = {
      __proto__: null,
      __wbg___wbindgen_boolean_get_fa956cfa2d1bd751: function(arg0) {
        const v = arg0;
        const ret = typeof v === "boolean" ? v : void 0;
        return isLikeNone(ret) ? 16777215 : ret ? 1 : 0;
      },
      __wbg___wbindgen_debug_string_c25d447a39f5578f: function(arg0, arg1) {
        const ret = debugString(arg1);
        const ptr1 = passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        const len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
      },
      __wbg___wbindgen_is_function_1ff95bcc5517c252: function(arg0) {
        const ret = typeof arg0 === "function";
        return ret;
      },
      __wbg___wbindgen_is_null_ea9085d691f535d3: function(arg0) {
        const ret = arg0 === null;
        return ret;
      },
      __wbg___wbindgen_is_object_a27215656b807791: function(arg0) {
        const val = arg0;
        const ret = typeof val === "object" && val !== null;
        return ret;
      },
      __wbg___wbindgen_is_undefined_c05833b95a3cf397: function(arg0) {
        const ret = arg0 === void 0;
        return ret;
      },
      __wbg___wbindgen_number_get_394265ed1e1b84ee: function(arg0, arg1) {
        const obj = arg1;
        const ret = typeof obj === "number" ? obj : void 0;
        getDataViewMemory0().setFloat64(arg0 + 8 * 1, isLikeNone(ret) ? 0 : ret, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, !isLikeNone(ret), true);
      },
      __wbg___wbindgen_string_get_b0ca35b86a603356: function(arg0, arg1) {
        const obj = arg1;
        const ret = typeof obj === "string" ? obj : void 0;
        var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
        var len1 = WASM_VECTOR_LEN;
        getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
        getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
      },
      __wbg___wbindgen_throw_344f42d3211c4765: function(arg0, arg1) {
        throw new Error(getStringFromWasm0(arg0, arg1));
      },
      __wbg__wbg_cb_unref_fffb441def202758: function(arg0) {
        arg0._wbg_cb_unref();
      },
      __wbg_apply_3ac86a26fdb56c05: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = arg0.apply(arg1, arg2);
          return ret;
        }, arguments);
      },
      __wbg_arrayBuffer_3b637f0fa65c5351: function() {
        return handleError(function(arg0) {
          const ret = arg0.arrayBuffer();
          return ret;
        }, arguments);
      },
      __wbg_body_18c9f2ac15ead4b2: function(arg0) {
        const ret = arg0.body;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
      },
      __wbg_call_a6e5c5dce5018821: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = arg0.call(arg1, arg2);
          return ret;
        }, arguments);
      },
      __wbg_encodeURIComponent_d0140ae6e13eb27b: function(arg0, arg1) {
        const ret = encodeURIComponent(getStringFromWasm0(arg0, arg1));
        return ret;
      },
      __wbg_fetch_6ecc661950e58d49: function(arg0, arg1) {
        const ret = arg0.fetch(arg1);
        return ret;
      },
      __wbg_from_13e323c65fc8f464: function(arg0) {
        const ret = Array.from(arg0);
        return ret;
      },
      __wbg_getReader_7455d080fa48369b: function(arg0) {
        const ret = arg0.getReader();
        return ret;
      },
      __wbg_get_18e0163e38e5048d: function() {
        return handleError(function(arg0, arg1, arg2, arg3) {
          const ret = arg1.get(getStringFromWasm0(arg2, arg3));
          var ptr1 = isLikeNone(ret) ? 0 : passStringToWasm0(ret, wasm.__wbindgen_malloc, wasm.__wbindgen_realloc);
          var len1 = WASM_VECTOR_LEN;
          getDataViewMemory0().setInt32(arg0 + 4 * 1, len1, true);
          getDataViewMemory0().setInt32(arg0 + 4 * 0, ptr1, true);
        }, arguments);
      },
      __wbg_get_78f252d074a84d0b: function() {
        return handleError(function(arg0, arg1) {
          const ret = Reflect.get(arg0, arg1);
          return ret;
        }, arguments);
      },
      __wbg_get_unchecked_6e0ad6d2a41b06f6: function(arg0, arg1) {
        const ret = arg0[arg1 >>> 0];
        return ret;
      },
      __wbg_headers_cf9c80f30e2a4eff: function(arg0) {
        const ret = arg0.headers;
        return ret;
      },
      __wbg_instanceof_ArrayBuffer_4480b9e0068a8adb: function(arg0) {
        let result;
        try {
          result = arg0 instanceof ArrayBuffer;
        } catch (_) {
          result = false;
        }
        const ret = result;
        return ret;
      },
      __wbg_instanceof_Uint8Array_309b927aaf7a3fc7: function(arg0) {
        let result;
        try {
          result = arg0 instanceof Uint8Array;
        } catch (_) {
          result = false;
        }
        const ret = result;
        return ret;
      },
      __wbg_instanceof_Window_05ba1ee4f6781663: function(arg0) {
        let result;
        try {
          result = arg0 instanceof Window;
        } catch (_) {
          result = false;
        }
        const ret = result;
        return ret;
      },
      __wbg_length_1f0964f4a5e2c6d8: function(arg0) {
        const ret = arg0.length;
        return ret;
      },
      __wbg_length_370319915dc99107: function(arg0) {
        const ret = arg0.length;
        return ret;
      },
      __wbg_new_0d809930cd1354c6: function() {
        return handleError(function() {
          const ret = new Headers();
          return ret;
        }, arguments);
      },
      __wbg_new_32b398fb48b6d94a: function() {
        const ret = new Array();
        return ret;
      },
      __wbg_new_aec3e25493d729fe: function(arg0, arg1) {
        try {
          var state0 = { a: arg0, b: arg1 };
          var cb0 = (arg02, arg12) => {
            const a = state0.a;
            state0.a = 0;
            try {
              return wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined_______true_(a, state0.b, arg02, arg12);
            } finally {
              state0.a = a;
            }
          };
          const ret = new Promise(cb0);
          return ret;
        } finally {
          state0.a = 0;
        }
      },
      __wbg_new_b667d279fd5aa943: function(arg0, arg1) {
        const ret = new Error(getStringFromWasm0(arg0, arg1));
        return ret;
      },
      __wbg_new_cd45aabdf6073e84: function(arg0) {
        const ret = new Uint8Array(arg0);
        return ret;
      },
      __wbg_new_da52cf8fe3429cb2: function() {
        const ret = new Object();
        return ret;
      },
      __wbg_new_from_slice_77cdfb7977362f3c: function(arg0, arg1) {
        const ret = new Uint8Array(getArrayU8FromWasm0(arg0, arg1));
        return ret;
      },
      __wbg_new_typed_1824d93f294193e5: function(arg0, arg1) {
        try {
          var state0 = { a: arg0, b: arg1 };
          var cb0 = (arg02, arg12) => {
            const a = state0.a;
            state0.a = 0;
            try {
              return wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined_______true_(a, state0.b, arg02, arg12);
            } finally {
              state0.a = a;
            }
          };
          const ret = new Promise(cb0);
          return ret;
        } finally {
          state0.a = 0;
        }
      },
      __wbg_new_with_str_and_init_d95cbe11ce28e65e: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = new Request(getStringFromWasm0(arg0, arg1), arg2);
          return ret;
        }, arguments);
      },
      __wbg_of_5f1b88183ddb5d94: function(arg0, arg1) {
        const ret = Array.of(arg0, arg1);
        return ret;
      },
      __wbg_prototypesetcall_4770620bbe4688a0: function(arg0, arg1, arg2) {
        Uint8Array.prototype.set.call(getArrayU8FromWasm0(arg0, arg1), arg2);
      },
      __wbg_push_d2ae3af0c1217ae6: function(arg0, arg1) {
        const ret = arg0.push(arg1);
        return ret;
      },
      __wbg_queueMicrotask_0ab5b2d2393e99b9: function(arg0) {
        const ret = arg0.queueMicrotask;
        return ret;
      },
      __wbg_queueMicrotask_6a09b7bc46549209: function(arg0) {
        queueMicrotask(arg0);
      },
      __wbg_race_ac5c7b465abcfa15: function(arg0) {
        const ret = Promise.race(arg0);
        return ret;
      },
      __wbg_read_8afa15f12a160ef8: function(arg0) {
        const ret = arg0.read();
        return ret;
      },
      __wbg_resolve_2191a4dfe481c25b: function(arg0) {
        const ret = Promise.resolve(arg0);
        return ret;
      },
      __wbg_setTimeout_725a27c387d005c7: function() {
        return handleError(function(arg0, arg1, arg2, arg3) {
          const ret = arg0.setTimeout(arg1, arg2, arg3);
          return ret;
        }, arguments);
      },
      __wbg_setTimeout_cfa2cf195c3738db: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = arg0.setTimeout(arg1, arg2);
          return ret;
        }, arguments);
      },
      __wbg_set_0de9c62c23d04ad5: function() {
        return handleError(function(arg0, arg1, arg2, arg3, arg4) {
          arg0.set(getStringFromWasm0(arg1, arg2), getStringFromWasm0(arg3, arg4));
        }, arguments);
      },
      __wbg_set_8535240470bf2500: function() {
        return handleError(function(arg0, arg1, arg2) {
          const ret = Reflect.set(arg0, arg1, arg2);
          return ret;
        }, arguments);
      },
      __wbg_set_body_029f2d171e0a005f: function(arg0, arg1) {
        arg0.body = arg1;
      },
      __wbg_set_headers_9c61d123c3ee1f10: function(arg0, arg1) {
        arg0.headers = arg1;
      },
      __wbg_set_method_5532d59b92d76467: function(arg0, arg1, arg2) {
        arg0.method = getStringFromWasm0(arg1, arg2);
      },
      __wbg_static_accessor_GLOBAL_4ef717fb391d88b7: function() {
        const ret = typeof global === "undefined" ? null : global;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
      },
      __wbg_static_accessor_GLOBAL_THIS_8d1badc68b5a74f4: function() {
        const ret = typeof globalThis === "undefined" ? null : globalThis;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
      },
      __wbg_static_accessor_SELF_146583524fe1469b: function() {
        const ret = typeof self === "undefined" ? null : self;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
      },
      __wbg_static_accessor_WINDOW_f2829a2234d7819e: function() {
        const ret = typeof window === "undefined" ? null : window;
        return isLikeNone(ret) ? 0 : addToExternrefTable0(ret);
      },
      __wbg_status_c45b3b9b3033184a: function(arg0) {
        const ret = arg0.status;
        return ret;
      },
      __wbg_then_16d107c451e9905d: function(arg0, arg1, arg2) {
        const ret = arg0.then(arg1, arg2);
        return ret;
      },
      __wbg_then_6ec10ae38b3e92f7: function(arg0, arg1) {
        const ret = arg0.then(arg1);
        return ret;
      },
      __wbindgen_cast_0000000000000001: function(arg0, arg1) {
        const ret = makeMutClosure(arg0, arg1, wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___wasm_bindgen_1e05ddb24c0b7df4___JsValue__core_9b3796e30d99ddb7___result__Result_____wasm_bindgen_1e05ddb24c0b7df4___JsError___true_);
        return ret;
      },
      __wbindgen_cast_0000000000000002: function(arg0) {
        const ret = arg0;
        return ret;
      },
      __wbindgen_cast_0000000000000003: function(arg0, arg1) {
        const ret = getStringFromWasm0(arg0, arg1);
        return ret;
      },
      __wbindgen_init_externref_table: function() {
        const table = wasm.__wbindgen_externrefs;
        const offset = table.grow(4);
        table.set(0, void 0);
        table.set(offset + 0, void 0);
        table.set(offset + 1, null);
        table.set(offset + 2, true);
        table.set(offset + 3, false);
      }
    };
    return {
      __proto__: null,
      "./libfw_client_bg.js": import0
    };
  }
  function wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___wasm_bindgen_1e05ddb24c0b7df4___JsValue__core_9b3796e30d99ddb7___result__Result_____wasm_bindgen_1e05ddb24c0b7df4___JsError___true_(arg0, arg1, arg2) {
    const ret = wasm.wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___wasm_bindgen_1e05ddb24c0b7df4___JsValue__core_9b3796e30d99ddb7___result__Result_____wasm_bindgen_1e05ddb24c0b7df4___JsError___true_(arg0, arg1, arg2);
    if (ret[1]) {
      throw takeFromExternrefTable0(ret[0]);
    }
  }
  function wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined_______true_(arg0, arg1, arg2, arg3) {
    wasm.wasm_bindgen_1e05ddb24c0b7df4___convert__closures_____invoke___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined___js_sys_bb6c79d0abe11c10___Function_fn_wasm_bindgen_1e05ddb24c0b7df4___JsValue_____wasm_bindgen_1e05ddb24c0b7df4___sys__Undefined_______true_(arg0, arg1, arg2, arg3);
  }
  var LibfwClientFinalization = typeof FinalizationRegistry === "undefined" ? { register: () => {
  }, unregister: () => {
  } } : new FinalizationRegistry((ptr) => wasm.__wbg_libfwclient_free(ptr, 1));
  function addToExternrefTable0(obj) {
    const idx = wasm.__externref_table_alloc();
    wasm.__wbindgen_externrefs.set(idx, obj);
    return idx;
  }
  var CLOSURE_DTORS = typeof FinalizationRegistry === "undefined" ? { register: () => {
  }, unregister: () => {
  } } : new FinalizationRegistry((state) => wasm.__wbindgen_destroy_closure(state.a, state.b));
  function debugString(val) {
    const type = typeof val;
    if (type == "number" || type == "boolean" || val == null) {
      return `${val}`;
    }
    if (type == "string") {
      return `"${val}"`;
    }
    if (type == "symbol") {
      const description = val.description;
      if (description == null) {
        return "Symbol";
      } else {
        return `Symbol(${description})`;
      }
    }
    if (type == "function") {
      const name = val.name;
      if (typeof name == "string" && name.length > 0) {
        return `Function(${name})`;
      } else {
        return "Function";
      }
    }
    if (Array.isArray(val)) {
      const length = val.length;
      let debug = "[";
      if (length > 0) {
        debug += debugString(val[0]);
      }
      for (let i = 1; i < length; i++) {
        debug += ", " + debugString(val[i]);
      }
      debug += "]";
      return debug;
    }
    const builtInMatches = /\[object ([^\]]+)\]/.exec(toString.call(val));
    let className;
    if (builtInMatches && builtInMatches.length > 1) {
      className = builtInMatches[1];
    } else {
      return toString.call(val);
    }
    if (className == "Object") {
      try {
        return "Object(" + JSON.stringify(val) + ")";
      } catch (_) {
        return "Object";
      }
    }
    if (val instanceof Error) {
      return `${val.name}: ${val.message}
${val.stack}`;
    }
    return className;
  }
  function getArrayU8FromWasm0(ptr, len) {
    ptr = ptr >>> 0;
    return getUint8ArrayMemory0().subarray(ptr / 1, ptr / 1 + len);
  }
  var cachedDataViewMemory0 = null;
  function getDataViewMemory0() {
    if (cachedDataViewMemory0 === null || cachedDataViewMemory0.buffer.detached === true || cachedDataViewMemory0.buffer.detached === void 0 && cachedDataViewMemory0.buffer !== wasm.memory.buffer) {
      cachedDataViewMemory0 = new DataView(wasm.memory.buffer);
    }
    return cachedDataViewMemory0;
  }
  function getStringFromWasm0(ptr, len) {
    return decodeText(ptr >>> 0, len);
  }
  var cachedUint8ArrayMemory0 = null;
  function getUint8ArrayMemory0() {
    if (cachedUint8ArrayMemory0 === null || cachedUint8ArrayMemory0.byteLength === 0) {
      cachedUint8ArrayMemory0 = new Uint8Array(wasm.memory.buffer);
    }
    return cachedUint8ArrayMemory0;
  }
  function handleError(f, args) {
    try {
      return f.apply(this, args);
    } catch (e) {
      const idx = addToExternrefTable0(e);
      wasm.__wbindgen_exn_store(idx);
    }
  }
  function isLikeNone(x) {
    return x === void 0 || x === null;
  }
  function makeMutClosure(arg0, arg1, f) {
    const state = { a: arg0, b: arg1, cnt: 1 };
    const real = (...args) => {
      state.cnt++;
      const a = state.a;
      state.a = 0;
      try {
        return f(a, state.b, ...args);
      } finally {
        state.a = a;
        real._wbg_cb_unref();
      }
    };
    real._wbg_cb_unref = () => {
      if (--state.cnt === 0) {
        wasm.__wbindgen_destroy_closure(state.a, state.b);
        state.a = 0;
        CLOSURE_DTORS.unregister(state);
      }
    };
    CLOSURE_DTORS.register(real, state, state);
    return real;
  }
  function passStringToWasm0(arg, malloc, realloc) {
    if (realloc === void 0) {
      const buf = cachedTextEncoder.encode(arg);
      const ptr2 = malloc(buf.length, 1) >>> 0;
      getUint8ArrayMemory0().subarray(ptr2, ptr2 + buf.length).set(buf);
      WASM_VECTOR_LEN = buf.length;
      return ptr2;
    }
    let len = arg.length;
    let ptr = malloc(len, 1) >>> 0;
    const mem = getUint8ArrayMemory0();
    let offset = 0;
    for (; offset < len; offset++) {
      const code = arg.charCodeAt(offset);
      if (code > 127) break;
      mem[ptr + offset] = code;
    }
    if (offset !== len) {
      if (offset !== 0) {
        arg = arg.slice(offset);
      }
      ptr = realloc(ptr, len, len = offset + arg.length * 3, 1) >>> 0;
      const view = getUint8ArrayMemory0().subarray(ptr + offset, ptr + len);
      const ret = cachedTextEncoder.encodeInto(arg, view);
      offset += ret.written;
      ptr = realloc(ptr, len, offset, 1) >>> 0;
    }
    WASM_VECTOR_LEN = offset;
    return ptr;
  }
  function takeFromExternrefTable0(idx) {
    const value = wasm.__wbindgen_externrefs.get(idx);
    wasm.__externref_table_dealloc(idx);
    return value;
  }
  var cachedTextDecoder = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
  cachedTextDecoder.decode();
  var MAX_SAFARI_DECODE_BYTES = 2146435072;
  var numBytesDecoded = 0;
  function decodeText(ptr, len) {
    numBytesDecoded += len;
    if (numBytesDecoded >= MAX_SAFARI_DECODE_BYTES) {
      cachedTextDecoder = new TextDecoder("utf-8", { ignoreBOM: true, fatal: true });
      cachedTextDecoder.decode();
      numBytesDecoded = len;
    }
    return cachedTextDecoder.decode(getUint8ArrayMemory0().subarray(ptr, ptr + len));
  }
  var cachedTextEncoder = new TextEncoder();
  if (!("encodeInto" in cachedTextEncoder)) {
    cachedTextEncoder.encodeInto = function(arg, view) {
      const buf = cachedTextEncoder.encode(arg);
      view.set(buf);
      return {
        read: arg.length,
        written: buf.length
      };
    };
  }
  var WASM_VECTOR_LEN = 0;
  var wasmModule;
  var wasmInstance;
  var wasm;
  function __wbg_finalize_init(instance, module) {
    wasmInstance = instance;
    wasm = instance.exports;
    wasmModule = module;
    cachedDataViewMemory0 = null;
    cachedUint8ArrayMemory0 = null;
    wasm.__wbindgen_start();
    return wasm;
  }
  async function __wbg_load(module, imports) {
    if (typeof Response === "function" && module instanceof Response) {
      if (typeof WebAssembly.instantiateStreaming === "function") {
        try {
          return await WebAssembly.instantiateStreaming(module, imports);
        } catch (e) {
          const validResponse = module.ok && expectedResponseType(module.type);
          if (validResponse && module.headers.get("Content-Type") !== "application/wasm") {
            console.warn("`WebAssembly.instantiateStreaming` failed because your server does not serve Wasm with `application/wasm` MIME type. Falling back to `WebAssembly.instantiate` which is slower. Original error:\n", e);
          } else {
            throw e;
          }
        }
      }
      const bytes = await module.arrayBuffer();
      return await WebAssembly.instantiate(bytes, imports);
    } else {
      const instance = await WebAssembly.instantiate(module, imports);
      if (instance instanceof WebAssembly.Instance) {
        return { instance, module };
      } else {
        return instance;
      }
    }
    function expectedResponseType(type) {
      switch (type) {
        case "basic":
        case "cors":
        case "default":
          return true;
      }
      return false;
    }
  }
  async function __wbg_init(module_or_path) {
    if (wasm !== void 0) return wasm;
    if (module_or_path !== void 0) {
      if (Object.getPrototypeOf(module_or_path) === Object.prototype) {
        ({ module_or_path } = module_or_path);
      } else {
        console.warn("using deprecated parameters for the initialization function; pass a single object instead");
      }
    }
    if (module_or_path === void 0) {
      module_or_path = new URL("libfw_client_bg.wasm", import_meta.url);
    }
    const imports = __wbg_get_imports();
    if (typeof module_or_path === "string" || typeof Request === "function" && module_or_path instanceof Request || typeof URL === "function" && module_or_path instanceof URL) {
      module_or_path = fetch(module_or_path);
    }
    const { instance, module } = await __wbg_load(await module_or_path, imports);
    return __wbg_finalize_init(instance, module);
  }

  // node_modules/.pnpm/libfw-client@0.1.3/node_modules/libfw-client/zip.js
  var CRC_TABLE = (() => {
    const table = new Uint32Array(256);
    for (let n = 0; n < 256; n += 1) {
      let c = n;
      for (let k = 0; k < 8; k += 1) {
        c = c & 1 ? 3988292384 ^ c >>> 1 : c >>> 1;
      }
      table[n] = c >>> 0;
    }
    return table;
  })();
  function crc32(bytes) {
    let crc = 4294967295;
    for (let i = 0; i < bytes.length; i += 1) {
      crc = CRC_TABLE[(crc ^ bytes[i]) & 255] ^ crc >>> 8;
    }
    return (crc ^ 4294967295) >>> 0;
  }
  function createZip(entries) {
    const encoder = new TextEncoder();
    const body = [];
    const central = [];
    let offset = 0;
    for (const entry of entries) {
      const nameBytes = encoder.encode(entry.name);
      const data = entry.data;
      const crc = crc32(data);
      const local = new Uint8Array(30 + nameBytes.length);
      const dv = new DataView(local.buffer);
      dv.setUint32(0, 67324752, true);
      dv.setUint16(4, 20, true);
      dv.setUint16(6, 0, true);
      dv.setUint16(8, 0, true);
      dv.setUint16(10, 0, true);
      dv.setUint16(12, 33, true);
      dv.setUint32(14, crc, true);
      dv.setUint32(18, data.length, true);
      dv.setUint32(22, data.length, true);
      dv.setUint16(26, nameBytes.length, true);
      dv.setUint16(28, 0, true);
      local.set(nameBytes, 30);
      body.push(local, data);
      central.push({ nameBytes, crc, size: data.length, offset });
      offset += local.length + data.length;
    }
    const dir = [];
    let cdSize = 0;
    for (const c of central) {
      const cd = new Uint8Array(46 + c.nameBytes.length);
      const dv = new DataView(cd.buffer);
      dv.setUint32(0, 33639248, true);
      dv.setUint16(4, 20, true);
      dv.setUint16(6, 20, true);
      dv.setUint16(8, 0, true);
      dv.setUint16(10, 0, true);
      dv.setUint16(12, 0, true);
      dv.setUint16(14, 33, true);
      dv.setUint32(16, c.crc, true);
      dv.setUint32(20, c.size, true);
      dv.setUint32(24, c.size, true);
      dv.setUint16(28, c.nameBytes.length, true);
      dv.setUint16(30, 0, true);
      dv.setUint16(32, 0, true);
      dv.setUint16(34, 0, true);
      dv.setUint16(36, 0, true);
      dv.setUint32(38, 0, true);
      dv.setUint32(42, c.offset, true);
      cd.set(c.nameBytes, 46);
      dir.push(cd);
      cdSize += cd.length;
    }
    const cdOffset = offset;
    const eocd = new Uint8Array(22);
    const edv = new DataView(eocd.buffer);
    edv.setUint32(0, 101010256, true);
    edv.setUint16(4, 0, true);
    edv.setUint16(6, 0, true);
    edv.setUint16(8, central.length, true);
    edv.setUint16(10, central.length, true);
    edv.setUint32(12, cdSize, true);
    edv.setUint32(16, cdOffset, true);
    edv.setUint16(20, 0, true);
    return new Blob([...body, ...dir, eocd], { type: "application/zip" });
  }

  // node_modules/.pnpm/libfw-client@0.1.3/node_modules/libfw-client/index.js
  var import_meta2 = {};
  var IDB_NAME = "libfw";
  var IDB_STORE = "resume";
  var LibfwError = class extends Error {
    /**
     * @param {string} message human-readable description
     * @param {string} [code] machine-readable category
     */
    constructor(message, code = "unknown") {
      super(message);
      this.name = "LibfwError";
      this.code = code;
    }
  };
  function toLibfwError(err) {
    if (err instanceof LibfwError) return err;
    if (err && typeof err === "object" && err.isLibfwError) {
      return new LibfwError(String(err.message || err), "wasm");
    }
    if (err && typeof err === "object" && err.message) {
      return new LibfwError(String(err.message), err.name === "AbortError" ? "abort" : "unknown");
    }
    return new LibfwError(String(err));
  }
  var Idb = {
    /** @returns {Promise<IDBDatabase>} */
    open() {
      return new Promise((resolve, reject) => {
        const req = indexedDB.open(IDB_NAME, 1);
        req.onupgradeneeded = () => {
          if (!req.result.objectStoreNames.contains(IDB_STORE)) {
            req.result.createObjectStore(IDB_STORE);
          }
        };
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(new LibfwError(`indexeddb open: ${req.error}`, "idb"));
      });
    },
    /**
     * @param {string} path virtual file path
     * @returns {Promise<object|null>} `{ etag, offset, size }` or `null`
     */
    async loadState(path) {
      try {
        const db = await Idb.open();
        return await new Promise((resolve, reject) => {
          const tx = db.transaction(IDB_STORE, "readonly");
          const req = tx.objectStore(IDB_STORE).get(path);
          req.onsuccess = () => resolve(req.result ?? null);
          req.onerror = () => reject(new LibfwError(`idb get: ${req.error}`, "idb"));
        });
      } catch (err) {
        if (err instanceof LibfwError) return null;
        throw err;
      }
    },
    /**
     * @param {string} path virtual file path
     * @param {object} state `{ etag, offset, size }`
     * @returns {Promise<void>}
     */
    async saveState(path, state) {
      const db = await Idb.open();
      await new Promise((resolve, reject) => {
        const tx = db.transaction(IDB_STORE, "readwrite");
        tx.objectStore(IDB_STORE).put(state, path);
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(new LibfwError(`idb put: ${tx.error}`, "idb"));
      });
    },
    /**
     * Delete every key whose `direction:` prefix matches (e.g. all
     * `download:*` keys) while leaving the other direction intact.
     * @param {string} direction `'upload'` | `'download'`
     * @returns {Promise<number>} number of records removed
     */
    async clearDirection(direction) {
      const db = await Idb.open();
      const prefix = `${direction}:`;
      const keys = await new Promise((resolve, reject) => {
        const tx = db.transaction(IDB_STORE, "readonly");
        const req = tx.objectStore(IDB_STORE).getAllKeys();
        req.onsuccess = () => resolve(req.result);
        req.onerror = () => reject(new LibfwError(`idb keys: ${req.error}`, "idb"));
      });
      const matches = keys.filter((key) => String(key).startsWith(prefix));
      if (matches.length === 0) return 0;
      await new Promise((resolve, reject) => {
        const tx = db.transaction(IDB_STORE, "readwrite");
        const store = tx.objectStore(IDB_STORE);
        for (const key of matches) store.delete(key);
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(new LibfwError(`idb clear direction: ${tx.error}`, "idb"));
      });
      return matches.length;
    },
    /**
     * Wipe the whole resume store.
     * @returns {Promise<void>}
     */
    async clear() {
      const db = await Idb.open();
      await new Promise((resolve, reject) => {
        const tx = db.transaction(IDB_STORE, "readwrite");
        tx.objectStore(IDB_STORE).clear();
        tx.oncomplete = () => resolve();
        tx.onerror = () => reject(new LibfwError(`idb clear: ${tx.error}`, "idb"));
      });
    }
  };
  function splitPath(path) {
    return String(path).split("/").filter((s) => s.length > 0);
  }
  var BUNDLE_SCRIPT_SRC = typeof document !== "undefined" && document.currentScript && document.currentScript.src ? document.currentScript.src : null;
  var LibfwClient2 = class {
    /**
     * @param {object} [options]
     * @param {string} [options.baseUrl=''] base URL the server routes are mounted under
     * @param {number} [options.concurrency=4] max concurrent file transfers
     * @param {boolean} [options.compress=true] negotiate zrip compression
     * @param {number} [options.chunkSize=2097152] upload chunk size in bytes
     * @param {number} [options.maxRetries=3] retries per chunk/file before failing
     * @param {number} [options.baseRetryDelayMs=500] initial backoff (ms)
     * @param {number} [options.maxRetryDelayMs=30000] backoff ceiling (ms)
     * @param {number} [options.timeoutMs=60000] per-request timeout (ms)
     * @param {string} [options.wasmUrl] explicit URL of `libfw_client_bg.wasm`;
     *        when omitted it is resolved automatically for both ESM and
     *        classic-`<script>`/UMD consumers (see {@link LibfwClient#_wasmUrl})
     * @param {'auto'|'fs'|'browser'} [options.downloadMode='auto'] how downloads
     *        reach the user's disk: `'fs'` uses the File System Access API
     *        (`showDirectoryPicker`); `'browser'` buffers each file and triggers
     *        a traditional browser download (folders are packed into a `.zip`);
     *        `'auto'` picks `'fs'` when the API exists, otherwise `'browser'`.
     * @param {number} [options.maxFallbackBytes=536870912] memory cap (bytes)
     *        for the in-memory `'browser'` download fallback. Each file's size
     *        (and the cumulative buffered total) is pre-checked against it
     *        before any bytes are buffered; a download that would exceed it is
     *        rejected with a `too-large` error instead of risking an OOM.
     *        `0` disables the limit.
     * @param {(event: {type: string, done: number, total: number, path?: string, error?: string}) => void} [options.onEvent]
     *        optional progress/state listener
     */
    constructor(options = {}) {
      this._options = {
        baseUrl: "",
        concurrency: 4,
        compress: true,
        chunkSize: 2 * 1024 * 1024,
        maxRetries: 3,
        baseRetryDelayMs: 500,
        maxRetryDelayMs: 3e4,
        timeoutMs: 6e4,
        wasmUrl: null,
        downloadMode: "auto",
        maxFallbackBytes: 512 * 1024 * 1024,
        onEvent: null,
        ...options
      };
      this._engine = null;
      this._initPromise = null;
      this._dirHandle = null;
      this._fileHandles = /* @__PURE__ */ new Map();
      this._writables = /* @__PURE__ */ new Map();
      this._uploadFiles = /* @__PURE__ */ new Map();
      this._uploadPlan = [];
      this._fallback = null;
    }
    // ------------------------------------------------------------------ setup
    /**
     * Lazily initialise the WASM engine (idempotent).
     * @returns {Promise<WasmEngine>}
     * @private
     */
    async _ready() {
      if (this._engine) return this._engine;
      if (!this._initPromise) {
        this._initPromise = __wbg_init({ module_or_path: this._wasmUrl() }).catch((err) => {
          this._initPromise = null;
          throw toLibfwError(err);
        });
      }
      await this._initPromise;
      const engine = new LibfwClient({
        concurrency: this._options.concurrency,
        compress: this._options.compress,
        chunkSize: this._options.chunkSize,
        maxRetries: this._options.maxRetries,
        baseRetryDelayMs: this._options.baseRetryDelayMs,
        maxRetryDelayMs: this._options.maxRetryDelayMs,
        timeoutMs: this._options.timeoutMs
      });
      engine.set_callbacks(this._makeCallbacks());
      this._engine = engine;
      return engine;
    }
    /**
     * Resolve the `.wasm` file URL without relying on `import.meta` (which is
     * ESM-only and a parse error in a classic `<script>`).
     *
     * Order: explicit `wasmUrl` option → classic-script `document.currentScript`
     * → ESM `import.meta.url`. The `wasmUrl` option is the escape hatch for
     * deployments where neither auto-detection applies.
     * @returns {string|URL}
     * @private
     */
    _wasmUrl() {
      if (this._options.wasmUrl) return this._options.wasmUrl;
      if (BUNDLE_SCRIPT_SRC) {
        return new URL("libfw_client_bg.wasm", BUNDLE_SCRIPT_SRC);
      }
      if (typeof import_meta2 !== "undefined" && import_meta2.url) {
        return new URL("./pkg/libfw_client_bg.wasm", import_meta2.url);
      }
      return "libfw_client_bg.wasm";
    }
    /**
     * Build the callbacks object handed to the WASM engine.
     * @returns {object}
     * @private
     */
    _makeCallbacks() {
      return {
        onFileStart: (path, size) => {
          if (this._fallback) {
            this._fallback.sizes.set(path, size);
            const max = this._maxFallbackBytes();
            if (max > 0) {
              if (size > max) {
                throw new LibfwError(
                  `file too large for browser download (${size} > ${max} bytes): ${path}`,
                  "too-large"
                );
              }
              this._fallback.total += size;
              if (this._fallback.total > max) {
                throw new LibfwError(
                  `browser download would buffer more than the ${max}-byte in-memory limit`,
                  "too-large"
                );
              }
            }
          }
          this._emit({ type: "fileStart", path, done: 0, total: size });
        },
        onWriteChunk: (path, offset, data) => this._onWriteChunk(path, offset, data),
        onFileCompleted: (path) => this._onFileCompleted(path),
        onProgress: (done, total) => this._emit({ type: "progress", done, total }),
        loadState: (direction, path) => Idb.loadState(`${direction}:${path}`),
        saveState: (direction, path, state) => {
          if (direction === "download" && this._fallback) return Promise.resolve();
          return Idb.saveState(`${direction}:${path}`, state);
        },
        getFileList: () => this._getFileList(),
        readFile: (path, offset, length) => this._readFile(path, offset, length),
        log: (msg) => {
          if (typeof console !== "undefined") console.debug(`[libfw] ${msg}`);
        }
      };
    }
    /** @private */
    _emit(event) {
      if (typeof this._options.onEvent === "function") {
        try {
          this._options.onEvent(event);
        } catch {
        }
      }
    }
    /**
     * Whether the File System Access API is available in this browser.
     * @returns {boolean}
     * @private
     */
    _supportsFsAccess() {
      return typeof window !== "undefined" && typeof window.showDirectoryPicker === "function" && typeof FileSystemFileHandle !== "undefined" && typeof FileSystemDirectoryHandle !== "undefined";
    }
    /**
     * Resolve the effective download mode from the `downloadMode` option:
     * an explicit `'fs'`/`'browser'` wins; `'auto'` falls back to the browser
     * download when the File System Access API is missing.
     * @returns {'fs'|'browser'}
     * @private
     */
    _effectiveMode() {
      const mode = this._options.downloadMode || "auto";
      if (mode === "fs" || mode === "browser") return mode;
      return this._supportsFsAccess() ? "fs" : "browser";
    }
    // ------------------------------------------------------------ downloads
    /**
     * Download a whole folder from the server.
     *
     * With the File System Access API available the folder is streamed into a
     * user-selected local directory (`showDirectoryPicker`), preserving the
     * structure through one `createWritable()` per file. Without FS API (or
     * with `downloadMode: 'browser'`) the folder is buffered in memory, packed
     * into a `.zip` and saved via a traditional browser download — no manual
     * feature detection needed by the caller.
     *
     * @param {string} token bearer token
     * @param {string} [dirPath=''] virtual server path to download (root by default)
     * @returns {Promise<number>} total bytes transferred
     * @throws {LibfwError}
     */
    async downloadFolder(token, dirPath = "") {
      const engine = await this._ready();
      if (this._effectiveMode() === "browser") {
        return this._downloadViaBrowser(engine, token, dirPath, true);
      }
      this._dirHandle = await window.showDirectoryPicker();
      this._fileHandles.clear();
      try {
        return await engine.download_folder(this._options.baseUrl, token, dirPath);
      } catch (err) {
        throw toLibfwError(err);
      } finally {
        await this._flushWritables();
        await this._syncResumeOffsets();
      }
    }
    /**
     * Download a single file from the server at `filePath`.
     *
     * With the File System Access API available the file is streamed into the
     * directory chosen via `showDirectoryPicker()`. Without FS API (or with
     * `downloadMode: 'browser'`) the file is buffered and saved through a
     * traditional browser download.
     *
     * @param {string} token bearer token
     * @param {string} filePath virtual server path of the file to download
     * @returns {Promise<number>} total bytes transferred
     * @throws {LibfwError}
     */
    async downloadFile(token, filePath) {
      const engine = await this._ready();
      if (!filePath) throw new LibfwError("downloadFile requires a file path", "path");
      if (this._effectiveMode() === "browser") {
        return this._downloadViaBrowser(engine, token, filePath, false);
      }
      this._dirHandle = await window.showDirectoryPicker();
      this._fileHandles.clear();
      try {
        return await engine.download_file(this._options.baseUrl, token, filePath);
      } catch (err) {
        throw toLibfwError(err);
      } finally {
        await this._flushWritables();
        await this._syncResumeOffsets();
      }
    }
    /**
     * Buffer-chunk fallback download used when the File System Access API is
     * unavailable (or `downloadMode: 'browser'`).
     *
     * `onWriteChunk` chunks are collected per path in memory (the engine keeps
     * calling them in order). When the transfer finishes: a single file is
     * emitted as a `Blob` and saved via a normal browser download; a folder is
     * packed into a `.zip` (STORE method) and downloaded. Progress/state events
     * keep flowing as usual. Note this buffers the whole transfer in memory —
     * the cost of not having FS API to stream to disk.
     *
     * @param {WasmEngine} engine
     * @param {string} token
     * @param {string} path virtual path
     * @param {boolean} isFolder
     * @returns {Promise<number>} total bytes transferred
     * @private
     */
    async _downloadViaBrowser(engine, token, path, isFolder) {
      this._fallback = { isFolder, buffers: /* @__PURE__ */ new Map(), order: [], sizes: /* @__PURE__ */ new Map(), total: 0 };
      try {
        const total = isFolder ? await engine.download_folder(this._options.baseUrl, token, path) : await engine.download_file(this._options.baseUrl, token, path);
        const { buffers, order, sizes } = this._fallback;
        if (isFolder) {
          const entries = [];
          for (const p of order) {
            entries.push({
              name: this._safeEntryName(p),
              data: this._concatBuffers(buffers.get(p) || [])
            });
          }
          for (const p of sizes.keys()) {
            if (!buffers.has(p)) {
              entries.push({ name: this._safeEntryName(p), data: new Uint8Array(0) });
            }
          }
          this._triggerBrowserDownload(createZip(entries), this._archiveName(path));
        } else {
          const data = this._concatBuffers(buffers.get(path) || []);
          this._triggerBrowserDownload(new Blob([data], { type: "application/octet-stream" }), this._downloadName(path));
        }
        return total;
      } catch (err) {
        throw toLibfwError(err);
      } finally {
        this._fallback = null;
      }
    }
    /**
     * Concatenate buffered chunks into one `Uint8Array`.
     * @param {Uint8Array[]} bufs
     * @returns {Uint8Array}
     * @private
     */
    _concatBuffers(bufs) {
      if (bufs.length === 0) return new Uint8Array(0);
      if (bufs.length === 1) return bufs[0];
      const len = bufs.reduce((n, b) => n + b.length, 0);
      const out = new Uint8Array(len);
      let off = 0;
      for (const b of bufs) {
        out.set(b, off);
        off += b.length;
      }
      return out;
    }
    /**
     * Strip a leading `/` so an entry path is archive/OS friendly.
     * @param {string} path
     * @returns {string}
     * @private
     */
    _cleanPath(path) {
      return String(path).replace(/^\/+/, "");
    }
    /**
     * Validate a virtual path for use as a ZIP entry name, rejecting any
     * traversal (`..`), absolute/drive-letter prefixes or Windows-style
     * separators that could escape the archive on extraction (zip-slip).
     * @param {string} path
     * @returns {string}
     * @private
     */
    _safeEntryName(path) {
      const cleaned = this._cleanPath(path);
      const segs = String(cleaned).split("/");
      if (segs.some((seg) => seg === ".." || seg.includes("\\") || /^[a-zA-Z]:/.test(seg))) {
        throw new LibfwError(`unsafe path in download: ${path}`, "path");
      }
      return cleaned;
    }
    /**
     * The configured in-memory cap for the browser-download fallback.
     * @returns {number} 0 disables the limit.
     * @private
     */
    _maxFallbackBytes() {
      const max = Number(this._options.maxFallbackBytes);
      return Number.isFinite(max) && max > 0 ? max : 0;
    }
    /**
     * Derive a safe file name from a virtual path.
     * @param {string} path
     * @returns {string}
     * @private
     */
    _downloadName(path) {
      const name = this._cleanPath(path).split("/").pop();
      return name || "download";
    }
    /**
     * Derive the `.zip` archive name for a folder download.
     * @param {string} path
     * @returns {string}
     * @private
     */
    _archiveName(path) {
      const base = this._cleanPath(path).split("/").pop() || "download";
      return `${base.replace(/[^\w.\- ]+/g, "_") || "download"}.zip`;
    }
    /**
     * Trigger a traditional browser download via a temporary `<a download>`.
     * @param {Blob} blob
     * @param {string} filename
     * @returns {void}
     * @private
     */
    _triggerBrowserDownload(blob, filename) {
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 6e4);
    }
    /**
     * Stream a decompressed chunk to disk, keeping memory bounded regardless
     * of file size (no whole-file buffering).
     *
     * The destination writable is opened exactly once per file and written in
     * **append mode** (`writable.write(data)` without an explicit `position`).
     * The engine awaits this callback, so chunks for a file arrive strictly in
     * order, making append writes correct for both fresh and resumed
     * downloads. Crucially, this avoids per-write
     * `{ type: 'write', position }` calls, which in Chromium can spawn a fresh
     * `.crswap` swap file per write and leave the target file empty on close —
     * the single `createWritable()` + sequential writes + one `close()` below
     * commits the swap file atomically.
     * @param {string} path virtual path
     * @param {number} offset byte offset (informational; writes append)
     * @param {Uint8Array} data decompressed chunk
     * @returns {Promise<void>}
     * @private
     */
    async _onWriteChunk(path, offset, data) {
      if (this._fallback) {
        let bufs = this._fallback.buffers.get(path);
        if (!bufs) {
          bufs = [];
          this._fallback.buffers.set(path, bufs);
          this._fallback.order.push(path);
        }
        bufs.push(data);
        return;
      }
      let entry = this._writables.get(path);
      if (!entry) {
        const { dir, name, handle } = await this._ensureFileHandle(path);
        this._fileHandles.set(path, handle);
        const isResume = offset > 0;
        if (!isResume) {
          await this._removeSwapFile(dir, name);
        }
        const writable = await handle.createWritable(
          isResume ? { keepExistingData: true } : void 0
        );
        entry = { writable, dir, name };
        this._writables.set(path, entry);
      }
      await entry.writable.write(data);
    }
    /**
     * Close the destination writable once a file's transfer completes.
     * @param {string} path virtual path
     * @returns {Promise<void>}
     * @private
     */
    async _onFileCompleted(path) {
      if (this._fallback) {
        this._emit({ type: "fileCompleted", path });
        return;
      }
      await this._closeWritable(path);
      this._emit({ type: "fileCompleted", path });
    }
    /**
     * Close (and forget) a file's writable, atomically committing the swap
     * file to its final name. Best-effort so failure/abort never throws.
     * @param {string} path virtual path
     * @returns {Promise<void>}
     * @private
     */
    async _closeWritable(path) {
      const entry = this._writables.get(path);
      if (entry) {
        this._writables.delete(path);
        try {
          await entry.writable.close();
        } catch {
        }
      }
    }
    /**
     * Resolve (and create, if needed) the file handle for a virtual path,
     * creating any parent directories along the way.
     * @param {string} path
     * @returns {Promise<{dir: FileSystemDirectoryHandle, name: string, handle: FileSystemFileHandle}>}
     * @private
     */
    async _ensureFileHandle(path) {
      const segments = splitPath(path);
      if (segments.length === 0) {
        throw new LibfwError(`invalid download path: ${path}`, "path");
      }
      let dir = this._dirHandle;
      for (let i = 0; i < segments.length - 1; i += 1) {
        dir = await dir.getDirectoryHandle(segments[i], { create: true });
      }
      const name = segments[segments.length - 1];
      const handle = await dir.getFileHandle(name, { create: true });
      return { dir, name, handle };
    }
    /**
     * Delete a leftover Chromium swap file (`.<name>.crswap`) next to a file,
     * ignoring any error (no swap file, or permission denied).
     * @param {FileSystemDirectoryHandle} dir parent directory
     * @param {string} name target file name
     * @returns {Promise<void>}
     * @private
     */
    async _removeSwapFile(dir, name) {
      try {
        await dir.removeEntry(`.${name}.crswap`, { recursive: false });
      } catch {
      }
    }
    /**
     * Close all still-open writable streams (flush to disk). Called on
     * success, failure or cancellation of a transfer.
     * @returns {Promise<void>}
     * @private
     */
    async _flushWritables() {
      const pending = [...this._writables.entries()].map(async ([path, entry]) => {
        this._writables.delete(path);
        try {
          await entry.writable.close();
        } catch {
        }
      });
      await Promise.allSettled(pending);
    }
    /**
     * Reconcile persisted download resume offsets with the bytes actually
     * committed to disk.
     *
     * `createWritable()` only commits to the real file on `close()`, so an
     * interrupted download's on-disk length can be ahead of (or behind) the
     * engine's periodically-saved offset. Overwriting each file's stored
     * offset with its real size keeps the append-based resume consistent:
     * the next transfer resumes exactly where the file on disk ends.
     * @returns {Promise<void>}
     * @private
     */
    async _syncResumeOffsets() {
      for (const [path, handle] of this._fileHandles) {
        try {
          const file = await handle.getFile();
          const size = file.size;
          const state = await Idb.loadState(`download:${path}`);
          if (state && typeof state.etag === "string") {
            await Idb.saveState(`download:${path}`, { ...state, offset: size, size });
          }
        } catch {
        }
      }
      this._fileHandles.clear();
    }
    // -------------------------------------------------------------- uploads
    /**
     * Upload files to the server.
     *
     * If `files` is omitted, `showDirectoryPicker()` is used to select a
     * local folder whose structure is mirrored on the server. Otherwise
     * `files` may be a `FileList`, an array of `File`s, or an array of
     * `{ path, size, mtime }` plan entries (when you want to drive reading
     * yourself).
     *
     * @param {string} token bearer token
     * @param {FileList|File[]|Array<{path:string,size:number,mtime:number}>} [files]
     * @returns {Promise<number>} total bytes uploaded
     * @throws {LibfwError}
     */
    async upload(token, files) {
      const engine = await this._ready();
      this._uploadFiles.clear();
      this._uploadPlan = [];
      if (files === void 0 || files === null) {
        if (typeof window === "undefined" || typeof window.showDirectoryPicker !== "function") {
          throw new LibfwError("File System Access API is not available in this browser", "unsupported");
        }
        const dir = await window.showDirectoryPicker();
        this._dirHandle = dir;
        this._uploadPlan = await this._collectDirectoryFiles(dir, "");
      } else {
        this._uploadPlan = await this._collectProvidedFiles(files);
      }
      try {
        return await engine.upload(this._options.baseUrl, token);
      } catch (err) {
        throw toLibfwError(err);
      }
    }
    /**
     * Walk a directory handle and build the upload plan.
     * @param {FileSystemDirectoryHandle} dir
     * @param {string} prefix virtual path prefix
     * @returns {Promise<Array<{path:string,size:number,mtime:number}>>}
     * @private
     */
    async _collectDirectoryFiles(dir, prefix) {
      const plan = [];
      for await (const [name, handle] of dir.entries()) {
        const path = prefix ? `${prefix}/${name}` : name;
        if (handle.kind === "directory") {
          plan.push(...await this._collectDirectoryFiles(handle, path));
        } else {
          const file = await handle.getFile();
          this._uploadFiles.set(path, file);
          plan.push({ path, size: file.size, mtime: Math.floor(file.lastModified / 1e3) });
        }
      }
      plan.sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
      return plan;
    }
    /**
     * Build the upload plan from a FileList / File[] / plan array.
     * @param {FileList|File[]|Array} files
     * @returns {Promise<Array<{path:string,size:number,mtime:number}>>}
     * @private
     */
    async _collectProvidedFiles(files) {
      if (Array.isArray(files) && files.length > 0 && typeof files[0] === "object" && files[0] !== null && "path" in files[0] && !(files[0] instanceof File)) {
        return files.map((f) => ({
          path: String(f.path),
          size: Number(f.size) || 0,
          mtime: Number(f.mtime) || 0
        }));
      }
      const list = Array.from(files || []);
      const plan = [];
      for (const file of list) {
        if (!(file instanceof File)) continue;
        const path = file.webkitRelativePath || file.name;
        this._uploadFiles.set(path, file);
        plan.push({ path, size: file.size, mtime: Math.floor(file.lastModified / 1e3) });
      }
      plan.sort((a, b) => a.path < b.path ? -1 : a.path > b.path ? 1 : 0);
      return plan;
    }
    /**
     * Engine callback: current upload plan.
     * @returns {Promise<Array<{path:string,size:number,mtime:number}>>}
     * @private
     */
    async _getFileList() {
      return this._uploadPlan;
    }
    /**
     * Engine callback: read `length` bytes of an upload file at `offset`.
     * @param {string} path
     * @param {number} offset
     * @param {number} length
     * @returns {Promise<Uint8Array>}
     * @private
     */
    async _readFile(path, offset, length) {
      const file = this._uploadFiles.get(path);
      if (!file) {
        throw new LibfwError(`upload source not found: ${path}`, "storage");
      }
      const blob = file.slice(offset, offset + length);
      const buffer = await blob.arrayBuffer();
      return new Uint8Array(buffer);
    }
    // ------------------------------------------------------------- controls
    /** Pause the active transfer (state → `paused`). */
    pause() {
      if (this._engine) this._engine.pause();
    }
    /** Resume a paused transfer. */
    resume() {
      if (this._engine) this._engine.resume();
    }
    /** Cancel the active transfer (state → `failed`). */
    cancel() {
      if (this._engine) this._engine.cancel();
    }
    /**
     * Current engine state: `idle | downloading | uploading | paused |
     * completed | failed`.
     * @returns {string}
     */
    state() {
      return this._engine ? this._engine.state() : "idle";
    }
    /**
     * Progress in `[0, 1]`.
     * @returns {number}
     */
    progress() {
      return this._engine ? this._engine.progress() : 0;
    }
    /**
     * Bytes transferred so far.
     * @returns {number}
     */
    doneBytes() {
      return this._engine ? this._engine.done_bytes() : 0;
    }
    /**
     * Total bytes to transfer.
     * @returns {number}
     */
    totalBytes() {
      return this._engine ? this._engine.total_bytes() : 0;
    }
    // ------------------------------------------------------------- resume store
    /**
     * Delete persisted resume state (IndexedDB).
     *
     * Pass a direction to wipe only that transfer's state, leaving the other
     * direction intact — the targeted replacement for clearing the whole store
     * before every transfer:
     *
     * - `await client.clearResumeStore('download')` — drop all download state.
     * - `await client.clearResumeStore('upload')` — drop all upload state.
     * - `await client.clearResumeStore()` — wipe everything (whole-store clear).
     *
     * @param {'upload'|'download'} [direction] restrict to one direction
     * @returns {Promise<number>} number of records removed
     */
    async clearResumeStore(direction) {
      if (direction !== void 0 && direction !== "upload" && direction !== "download") {
        throw new LibfwError(
          `clearResumeStore: expected 'upload' | 'download' | undefined, got ${JSON.stringify(direction)}`,
          "path"
        );
      }
      if (direction === void 0) {
        await Idb.clear();
        return 0;
      }
      return Idb.clearDirection(direction);
    }
  };
  var libfw_client_default = LibfwClient2;
  return __toCommonJS(libfw_client_exports);
})();
