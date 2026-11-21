// 크기 예산 검사: gzip 3KB 이하
import { readFileSync } from "node:fs";
import { gzipSync } from "node:zlib";

const BUDGET = 3 * 1024;
const js = readFileSync(new URL("../dist/t.js", import.meta.url));
const gz = gzipSync(js, { level: 9 }).length;
console.log(`t.js: ${js.length} B (gzip ${gz} B / 예산 ${BUDGET} B)`);
if (gz > BUDGET) {
  console.error("크기 예산 초과");
  process.exit(1);
}
