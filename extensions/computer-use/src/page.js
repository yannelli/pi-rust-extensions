(() => {
  const MAX_NODES = 1500;
  const nodes = [];
  const INTERACTIVE = new Set(["A", "BUTTON", "INPUT", "SELECT", "TEXTAREA", "SUMMARY", "OPTION", "LABEL"]);
  const TEXTUAL = new Set(["H1", "H2", "H3", "H4", "H5", "H6", "P", "LI", "TD", "TH", "DT", "DD", "CAPTION", "LEGEND", "FIGCAPTION", "IMG"]);
  const SKIP = new Set(["SCRIPT", "STYLE", "NOSCRIPT", "TEMPLATE", "SVG", "HEAD", "META", "LINK"]);
  const roleOf = (el) => {
    const explicit = el.getAttribute("role");
    if (explicit) return explicit.split(" ")[0].toLowerCase();
    const tag = el.tagName;
    if (tag === "A") return el.hasAttribute("href") ? "link" : "group";
    if (tag === "INPUT") {
      const type = (el.getAttribute("type") || "text").toLowerCase();
      if (["checkbox", "radio"].includes(type)) return type;
      if (["button", "submit", "reset", "image"].includes(type)) return "button";
      return "text_field";
    }
    if (tag === "TEXTAREA") return "text_area";
    if (tag === "SELECT") return "pop_up_button";
    if (tag === "IMG") return "image";
    if (/^H[1-6]$/.test(tag)) return "heading";
    if (el.isContentEditable) return "text_area";
    return tag.toLowerCase();
  };
  const directText = (el) => {
    let text = "";
    for (const child of el.childNodes) if (child.nodeType === 3) text += child.textContent;
    return text.replace(/\s+/g, " ").trim();
  };
  const labelOf = (el) => {
    const aria = el.getAttribute("aria-label") || el.getAttribute("alt") || el.getAttribute("title") || el.getAttribute("placeholder");
    if (aria) return aria.trim();
    if (el.labels && el.labels.length) return el.labels[0].innerText.trim();
    const text = INTERACTIVE.has(el.tagName) ? el.innerText : directText(el);
    return (text || "").replace(/\s+/g, " ").trim().slice(0, 400);
  };
  const visible = (el) => {
    const style = getComputedStyle(el);
    if (style.display === "none" || style.visibility === "hidden") return false;
    const rect = el.getBoundingClientRect();
    return rect.width > 0 && rect.height > 0;
  };
  const isInteractive = (el) =>
    INTERACTIVE.has(el.tagName) || el.hasAttribute("role") || el.hasAttribute("onclick") ||
    el.hasAttribute("tabindex") || el.isContentEditable;
  const editable = (el) =>
    (el.tagName === "INPUT" && roleOf(el) === "text_field") || el.tagName === "TEXTAREA" || el.isContentEditable;
  const toNode = (el) => {
    const rect = el.getBoundingClientRect();
    const index = nodes.push(el) - 1;
    const scrolls = el.scrollHeight > el.clientHeight + 4 && ["auto", "scroll"].includes(getComputedStyle(el).overflowY);
    const value = "value" in el && typeof el.value === "string" ? el.value : el.isContentEditable ? el.innerText : null;
    return {
      ref: "",
      role: roleOf(el),
      label: labelOf(el) || null,
      value: value ? value.slice(0, 4000) : null,
      identifier: el.id || null,
      frame: { x: rect.left, y: rect.top, w: rect.width, h: rect.height },
      actions: isInteractive(el) ? ["press"] : [],
      canPress: isInteractive(el) && !editable(el),
      canSetValue: editable(el),
      canFocus: typeof el.focus === "function" && (isInteractive(el) || editable(el)),
      canScroll: scrolls,
      focused: document.activeElement === el,
      path: [index],
      children: [],
    };
  };
  const textual = (el) => TEXTUAL.has(el.tagName) || (directText(el).length > 0 && el.children.length === 0);
  const walk = (el, parent) => {
    if (nodes.length >= MAX_NODES || SKIP.has(el.tagName) || !visible(el)) return;
    const keep = isInteractive(el) || textual(el);
    const node = keep ? toNode(el) : null;
    if (node) parent.children.push(node);
    if (el.tagName === "SELECT") return;
    for (const child of el.children) walk(child, node || parent);
    if (el.shadowRoot) for (const child of el.shadowRoot.children) walk(child, node || parent);
  };
  const body = document.body || document.documentElement;
  const root = {
    ref: "", role: "web_area", label: document.title || location.href, value: null, identifier: null,
    frame: { x: 0, y: 0, w: innerWidth, h: innerHeight }, actions: [], canPress: false, canSetValue: false,
    canFocus: false, canScroll: true, focused: false, path: [], children: [],
  };
  if (body) for (const child of body.children) walk(child, root);
  const node = (i) => {
    const el = window.__piCuNodes && window.__piCuNodes[i];
    if (!el || !el.isConnected) throw new Error("Browser element is stale. Observe the page again.");
    el.scrollIntoView({ block: "center", inline: "center" });
    return el;
  };
  window.__piCuNodes = nodes;
  window.__piCu = {
    press: (i) => { node(i).click(); return true; },
    focus: (i) => { node(i).focus(); return document.activeElement === node(i); },
    setText: (i, text) => {
      const el = node(i);
      el.focus();
      if (el.isContentEditable) el.innerText = text;
      else {
        const proto = el.tagName === "TEXTAREA" ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, "value").set.call(el, text);
      }
      el.dispatchEvent(new Event("input", { bubbles: true }));
      el.dispatchEvent(new Event("change", { bubbles: true }));
      return true;
    },
  };
  return { title: document.title, url: location.href, viewport: { w: innerWidth, h: innerHeight }, root };
})()
