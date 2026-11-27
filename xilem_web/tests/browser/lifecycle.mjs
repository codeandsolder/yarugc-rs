// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0
// Serve the wasm-bindgen output of the browser_lifecycle example to Chromium.
// Usage: node lifecycle.mjs <wasm-bindgen-output-directory> <chromium-executable>

import { createServer } from "node:http";
import { readFile, mkdtemp, rm } from "node:fs/promises";
import { join, resolve } from "node:path";
import { spawn } from "node:child_process";
import assert from "node:assert/strict";

const [directory, chromium] = process.argv.slice(2);
if (!directory || !chromium) {
    throw new Error("Specify the generated wasm-bindgen directory and Chromium executable");
}
const root = resolve(directory);
const html = '<!doctype html><html><body data-result="pending"><script type="module" src="/test.js"></script></body></html>';
const test = [
    'import init from "/browser_lifecycle.js";',
    'const node = id => document.getElementById(id);',
    'const check = (condition, detail) => { if (!condition) throw new Error(detail); };',
    'const wait = ms => new Promise(resolve => setTimeout(resolve, ms));',
    'const until = async predicate => { for (let n = 0; n < 100; n++) { if (predicate()) return; await wait(10); } throw new Error("Timed out"); };',
    'try {',
    '  await init();',
    '  await until(() => typeof globalThis.resolveRequest0 === "function" && node("target"));',
    '  const old = node("target");',
    '  check(old.tagName === "BUTTON", "initial custom element");',
    '  check(node("cascade").style.marginLeft === "21px", "initial longhand");',
    '  check(node("overwritten-checked").checked, "initial outermost boolean");',
    '  const circle = node("circle"), line = node("line"), rect = node("rect");',
    '  check(circle.r.baseVal.value === 3 && circle.cx.baseVal.value === 10, "initial circle geometry");',
    '  check(line.y1.baseVal.value === 2 && line.x2.baseVal.value === 3, "initial line geometry");',
    '  check(line.getAttribute("stroke-linecap") === "round" && line.getAttribute("stroke-linejoin") === "round", "initial stroke geometry");',
    '  check(rect.x.baseVal.value === 1 && rect.width.baseVal.value === 5, "initial rectangle geometry");',
    '  old.click();',
    '  check(node("count").textContent === "1", "initial click");',
    '  node("advance").click();',
    '  check(node("overwritten-checked").checked, "shadowed boolean change");',
    '  const current = node("target");',
    '  check(node("circle") === circle && circle.r.baseVal.value === 8 && circle.cx.baseVal.value === 15 && !circle.hasAttribute("height"), "retained circle geometry");',
    '  check(node("line") === line && line.y1.baseVal.value === 7 && line.x2.baseVal.value === 8, "retained line geometry");',
    '  check(line.getAttribute("stroke-linecap") === "square" && line.getAttribute("stroke-linejoin") === "miter" && line.getAttribute("stroke-miterlimit") === "6" && line.getAttribute("stroke-width") === "4" && line.getAttribute("stroke-dasharray") === "3 1" && line.getAttribute("stroke-dashoffset") === "2", "retained stroke geometry");',
    '  check(node("rect") === rect && rect.x.baseVal.value === 6 && rect.width.baseVal.value === 10, "retained rectangle geometry");',
    '  check(current !== old && current.tagName === "SPAN", "custom tag replacement");',
    '  check(current.className === "outer shared" || current.className === "shared outer", "classes restored on replacement");',
    '  check(current.style.color === "red", "style restored on replacement");',
    '  old.click();',
    '  check(node("count").textContent === "1", "old DOM listener removed");',
    '  current.click();',
    '  check(node("count").textContent === "2", "replacement click listener");',
    '  node("advance").click();',
    '  check(!node("overwritten-checked").checked, "changed outermost boolean");',
    '  current.click();',
    '  check(node("count").textContent === "2", "old event name removed on same DOM node");',
    '  check(node("cascade").style.marginTop === "8px" && node("cascade").style.marginLeft === "21px", "changed shorthand retains later longhand");',
    '  current.dispatchEvent(new Event("mousedown"));',
    '  check(node("count").textContent === "3", "changed event name registered");',
    '  node("reentrant").click();',
    '  await until(() => node("count").textContent === "14");',
    '  node("request").click();',
    '  await until(() => globalThis.droppedRequest0 === true);',
    '  globalThis.resolveRequest0("obsolete");',
    '  await wait(20);',
    '  check(node("output").textContent === "", "obsolete future rejected during debounce");',
    '  await until(() => typeof globalThis.resolveRequest1 === "function");',
    '  globalThis.resolveRequest1("current");',
    '  await until(() => node("output").textContent === "current");',
    '  document.body.dataset.result = "passed";',
    '} catch (error) {',
    '  document.body.dataset.result = "failed";',
    '  document.body.dataset.error = String(error);',
    '}',
].join("\n");

const server = createServer(async (request, response) => {
    try {
        if (request.url === "/") {
            response.writeHead(200, { "Content-Type": "text/html" }).end(html);
        } else if (request.url === "/test.js") {
            response.writeHead(200, { "Content-Type": "application/javascript" }).end(test);
        } else if (request.url === "/browser_lifecycle.js" || request.url === "/browser_lifecycle_bg.wasm") {
            const file = request.url.slice(1);
            response.writeHead(200, { "Content-Type": file.endsWith(".wasm") ? "application/wasm" : "application/javascript" });
            response.end(await readFile(join(root, file)));
        } else {
            response.writeHead(404).end();
        }
    } catch {
        response.writeHead(500).end();
    }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
const profile = await mkdtemp(join(root, "chromium-"));
try {
    const browser = spawn(chromium, [
        "--headless", "--disable-gpu", "--disable-background-networking",
        "--no-first-run", "--no-default-browser-check", "--disable-sync",
        "--user-data-dir=" + profile,
        "--dump-dom", "--virtual-time-budget=5000",
        "http://127.0.0.1:" + server.address().port + "/",
    ]);
    let output = "", errors = "";
    browser.stdout.on("data", chunk => output += chunk);
    browser.stderr.on("data", chunk => errors += chunk);
    const code = await new Promise((resolve, reject) => {
        browser.once("error", reject);
        browser.once("close", resolve);
    });
    assert.equal(code, 0, errors);
    assert.match(output, /data-result="passed"/, output + "\n" + errors);
    console.log("Browser lifecycle regression passed");
} finally {
    server.close();
    await rm(profile, { recursive: true, force: true });
}
