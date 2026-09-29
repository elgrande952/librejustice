// The one thing a dynamic import needs a host language for: imports
// `/redactor/lj_redact.js` and poses its promise as `ljRedactReady`.
window.ljRedactReady = (async () => {
  const module = await import("/redactor/lj_redact.js");
  await module.default();
  return module;
})();
