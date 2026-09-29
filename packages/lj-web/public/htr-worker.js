// One slow reader, holding the page it was last handed.
import init, { Htr } from "/redactor/htr/lj_htr.js";

let reader = null;
let sheet = null;

const answer = {
  async start({ module, networks }) {
    await init({ module_or_path: module });
    const [det, rec, dict, laia, syms, french, ...others] = networks;
    reader = new Htr(new Uint8Array(det), new Uint8Array(rec), dict, new Uint8Array(laia), syms, french, others);
  },
  sheet({ grey, width }) {
    sheet = { grey, width };
  },
  band({ index, count }) {
    return reader.band(sheet.grey, sheet.width, index, count);
  },
  segments({ map }) {
    return reader.segments(map, sheet.width, sheet.grey.length / sheet.width);
  },
  printed({ segments }) {
    return reader.printed(sheet.grey, sheet.width, segments);
  },
  batches({ segments, lines }) {
    return reader.batches(segments, lines);
  },
  hand({ segments }) {
    return reader.hand(sheet.grey, sheet.width, segments);
  },
  page({ lines }) {
    return reader.page(lines);
  },
};

onmessage = async ({ data }) => {
  try {
    postMessage({ id: data.id, result: await answer[data.kind](data) });
  } catch (error) {
    postMessage({ id: data.id, error: String(error?.message ?? error) });
  }
};
