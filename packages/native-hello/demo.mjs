import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { hello } = require("./index.js");

console.log(hello());
