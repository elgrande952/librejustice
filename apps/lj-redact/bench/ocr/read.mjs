// Reads every image listed in LIST with the tesseract.js build the redactor
// serves, writing one JSON line of words per image to OUT.
import { createScheduler, createWorker } from "tesseract.js";
import { appendFileSync, readFileSync, writeFileSync } from "node:fs";

const [list, out, langPath] = process.argv.slice(2);
const params = JSON.parse(process.env.LJ_OCR_PARAMS ?? "{}");
const images = readFileSync(list, "utf8").trim().split("\n");
const scheduler = createScheduler();
for (let i = 0; i < Number(process.env.LJ_OCR_WORKERS ?? 6); i++) {
  const worker = await createWorker("fra", 1, { langPath, gzip: false, cacheMethod: "none" });
  await worker.setParameters(params);
  scheduler.addWorker(worker);
}
writeFileSync(out, "");
await Promise.all(
  images.map(async (path) => {
    const { data } = await scheduler.addJob("recognize", path, {}, { blocks: true });
    const words = [];
    for (const block of data.blocks ?? [])
      for (const paragraph of block.paragraphs ?? [])
        for (const line of paragraph.lines ?? [])
          for (const w of line.words ?? []) words.push([w.text, w.bbox.x0, w.bbox.y0, w.bbox.x1, w.bbox.y1]);
    appendFileSync(out, JSON.stringify({ path, words }) + "\n");
  }),
);
await scheduler.terminate();
