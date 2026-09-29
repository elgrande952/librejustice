// Tesseract is Emscripten-built, so it is loaded the way it expects. The worker
// is kept between pages: creating one re-fetches the 1.5 MB of core and data.
window.ljOcrReady = (async () => {
  // The ESM build exports the whole library as its default and names nothing.
  const { createWorker } = (await import("/ocr/tesseract.esm.min.js")).default;
  let worker = null;
  // The worker holds its logger for its whole life, so the callback it calls is
  // this stable field and never the one handed to a single call.
  const reader = {
    progress: null,
    async words(image, page) {
      if (!worker) {
        worker = await createWorker("fra", 1, {
          // Without this, tesseract.js wraps its worker in a blob: URL, which
          // the site's `script-src 'self'` refuses.
          workerBlobURL: false,
          workerPath: "/ocr/worker.min.js",
          corePath: "/ocr/tesseract-core-simd-lstm.js",
          langPath: "/ocr",
          gzip: false,
          logger: (m) => reader.progress?.(m.status ?? "", m.progress ?? 0),
        });
      }
      const { data } = await worker.recognize(image, {}, { blocks: true });
      const words = [];
      let line = 0;
      for (const block of data.blocks ?? []) {
        for (const paragraph of block.paragraphs ?? []) {
          for (const row of paragraph.lines ?? []) {
            for (const word of row.words ?? []) {
              const b = word.bbox;
              words.push({
                page,
                line,
                text: word.text,
                x: b.x0,
                y: b.y0,
                width: b.x1 - b.x0,
                height: b.y1 - b.y0,
              });
            }
            line += 1;
          }
        }
      }
      return words;
    },
  };
  return reader;
})();
