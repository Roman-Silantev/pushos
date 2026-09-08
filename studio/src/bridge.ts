// Whether Studio can reach a running PushOS.
//
// Deliberately tiny and free of sample data, so asking the question costs a
// production build nothing.

/** Whether the Tauri bridge is present, which means PushOS can be reached. */
export function hasBridge(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

/**
 * Whether Studio should fall back to sample data.
 *
 * Only ever true in a development build with no bridge, which is exactly the
 * case when someone runs `npm run dev` to work on the layout. The bundler
 * resolves the first half to `false` in a production build and removes the
 * branch entirely.
 */
export function usePreview(): boolean {
  return import.meta.env.DEV && !hasBridge();
}
