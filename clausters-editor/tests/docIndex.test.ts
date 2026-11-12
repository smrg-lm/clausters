// The documentation's search: src/lib/docIndex.ts over two small books.

import { test } from "node:test";
import assert from "node:assert/strict";
import { DocIndex } from "../src/lib/docIndex.ts";

const BOOKS: Record<string, string> = {
  "guide.md": "# Guide\n\nStart here.\n\n## Buffers\n\nA buffer holds samples.\n\n```\n# not a heading\n```\n",
  "api.md": "# API\n\n## Server\n\nBoots the server.\n",
};

const index = () => DocIndex.build(Object.keys(BOOKS), async (p) => BOOKS[p]);

test("a word finds its section, with the line it is on", async () => {
  const [hit] = (await index()).search("samples");
  assert.equal(hit.path, "guide.md");
  assert.equal(hit.heading, "Buffers");
  assert.equal(hit.line, "A buffer holds samples.");
});

test("a heading weighs more than the text under another", async () => {
  const hits = (await index()).search("server");
  assert.equal(hits[0].heading, "Server");
});

test("a line inside a code fence is text, not a heading", async () => {
  const hits = (await index()).search("heading");
  assert.equal(hits.length, 1);
  assert.equal(hits[0].heading, "Buffers");
});

test("a book that cannot be read is left out, and the rest still searched", async () => {
  const found = await DocIndex.build(["gone.md", "api.md"], async (p) => {
    if (p === "gone.md") throw new Error("no such file");
    return BOOKS[p];
  });
  assert.equal(found.search("boots")[0].path, "api.md");
});
