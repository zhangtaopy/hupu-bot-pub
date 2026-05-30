/**
 * Header component setup — exports toggleDarkMode and related header state.
 */
export function setupHeader(store) {
  // Functions are already in store; this module exists for organization.
  // Direct re-exports so the intent is clear.
  return {
    toggleDarkMode: store.toggleDarkMode,
  };
}
