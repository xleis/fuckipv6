/* Shared WebUI component runtime. No framework and no innerHTML for user text. */
(() => {
  "use strict";

  const SVG_NS = "http://www.w3.org/2000/svg";
  const FOCUSABLE = [
    "a[href]",
    "button:not([disabled])",
    "input:not([disabled]):not([type=hidden])",
    "select:not([disabled])",
    "textarea:not([disabled])",
    "[tabindex]:not([tabindex='-1'])"
  ].join(",");

  const COMPONENTS = Object.freeze({
    button: "button",
    input: "input",
    select: "select",
    checkbox: "checkbox",
    radio: "radio",
    tabs: "tabs",
    breadcrumb: "breadcrumb",
    toolbar: "toolbar",
    table: "table",
    virtualList: "virtual-list",
    badge: "badge",
    statusBadge: "status-badge",
    capabilityRing: "capability-ring",
    dialog: "dialog",
    menu: "menu",
    contextMenu: "context-menu",
    tooltip: "tooltip",
    toast: "toast",
    emptyState: "empty-state",
    errorState: "error-state",
    permissionDeniedState: "permission-denied-state",
    unsupportedState: "unsupported-state"
  });

  const COMPONENT_NAMES = new Set(Object.values(COMPONENTS));
  const TONES = new Set(["neutral", "info", "success", "warning", "danger"]);
  const dialogs = new Set();
  const openMenus = new Set();
  let tooltipSequence = 0;
  let toastSequence = 0;
  let activeDialog = null;
  let dialogReturnFocus = null;

  function node(tag, className, text) {
    const element = document.createElement(tag);
    if (className) element.className = className;
    if (text !== undefined) element.textContent = String(text);
    return element;
  }

  function svgNode(tag, attributes = {}) {
    const element = document.createElementNS(SVG_NS, tag);
    for (const [name, value] of Object.entries(attributes)) {
      element.setAttribute(name, String(value));
    }
    return element;
  }

  function append(parent, children) {
    const list = Array.isArray(children) ? children : [children];
    for (const child of list.flat(Infinity)) {
      if (child === null || child === undefined || child === false) continue;
      parent.append(child instanceof Node ? child : document.createTextNode(String(child)));
    }
    return parent;
  }

  function setDataComponent(element, name) {
    if (!COMPONENT_NAMES.has(name)) {
      throw new TypeError(`Unknown UI component: ${String(name)}`);
    }
    element.dataset.component = name;
    return element;
  }

  function tone(value) {
    return TONES.has(value) ? value : "neutral";
  }

  function button(options = {}) {
    const element = node("button", "ui-button", options.label || "");
    setDataComponent(element, COMPONENTS.button);
    element.type = options.type || "button";
    element.dataset.variant = options.variant || "secondary";
    if (options.size) element.dataset.size = options.size;
    if (options.iconOnly) {
      element.dataset.iconOnly = "";
      if (!options.label && options.ariaLabel) element.setAttribute("aria-label", options.ariaLabel);
    }
    if (options.title) element.title = options.title;
    if (options.disabled) element.disabled = true;
    if (options.icon) append(element, options.icon);
    if (options.label) append(element, node("span", "", options.label));
    if (typeof options.onClick === "function") element.addEventListener("click", options.onClick);
    return element;
  }

  function input(options = {}) {
    const element = node("input", "ui-input");
    setDataComponent(element, COMPONENTS.input);
    element.type = options.type || "text";
    if (options.id) element.id = options.id;
    if (options.name) element.name = options.name;
    if (options.value !== undefined) element.value = String(options.value);
    if (options.placeholder) element.placeholder = options.placeholder;
    if (options.autocomplete) element.autocomplete = options.autocomplete;
    if (options.ariaLabel) element.setAttribute("aria-label", options.ariaLabel);
    if (options.description) element.setAttribute("aria-description", options.description);
    if (options.required) element.required = true;
    if (options.disabled) element.disabled = true;
    if (options.readOnly) element.readOnly = true;
    if (options.invalid) element.setAttribute("aria-invalid", "true");
    if (options.inputMode) element.inputMode = options.inputMode;
    if (typeof options.onInput === "function") element.addEventListener("input", options.onInput);
    return element;
  }

  function select(options = {}) {
    const element = node("select", "ui-select");
    setDataComponent(element, COMPONENTS.select);
    if (options.id) element.id = options.id;
    if (options.name) element.name = options.name;
    if (options.ariaLabel) element.setAttribute("aria-label", options.ariaLabel);
    if (options.description) element.setAttribute("aria-description", options.description);
    if (options.required) element.required = true;
    if (options.disabled) element.disabled = true;
    if (options.invalid) element.setAttribute("aria-invalid", "true");
    for (const option of options.options || []) {
      const item = node("option", "", option.label ?? option.value);
      item.value = String(option.value);
      if (option.disabled) item.disabled = true;
      if (option.value === options.value) item.selected = true;
      element.append(item);
    }
    if (typeof options.onChange === "function") element.addEventListener("change", options.onChange);
    return element;
  }

  function choice(type, options = {}) {
    const component = type === "radio" ? COMPONENTS.radio : COMPONENTS.checkbox;
    const root = node("label", type === "radio" ? "ui-radio" : "ui-check");
    setDataComponent(root, component);
    const control = node("input");
    control.type = type;
    if (options.id) control.id = options.id;
    if (options.name) control.name = options.name;
    if (options.value !== undefined) control.value = String(options.value);
    if (options.checked) control.checked = true;
    if (options.disabled) control.disabled = true;
    if (options.required) control.required = true;
    if (typeof options.onChange === "function") control.addEventListener("change", options.onChange);
    root.append(control, node("span", "", options.label || ""));
    return root;
  }

  function checkbox(options = {}) {
    return choice("checkbox", options);
  }

  function radio(options = {}) {
    return choice("radio", options);
  }

  function tabs(options = {}) {
    const root = node("div", "ui-tabs");
    setDataComponent(root, COMPONENTS.tabs);
    root.setAttribute("role", "tablist");
    root.setAttribute("aria-label", options.label || "Tabs");
    root.setAttribute("aria-orientation", options.orientation || "horizontal");
    const buttons = [];
    const panels = [];
    for (const [index, item] of (options.items || []).entries()) {
      const tab = node("button", "ui-tab", item.label || item.id);
      tab.type = "button";
      tab.id = item.tabId || `${item.id}-tab`;
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-controls", item.panelId || `${item.id}-panel`);
      tab.setAttribute("aria-selected", String(Boolean(item.selected)));
      tab.tabIndex = item.selected ? 0 : -1;
      if (item.disabled) tab.disabled = true;
      tab.addEventListener("click", () => activate(index));
      root.append(tab);
      buttons.push(tab);
      const panel = node("div", "ui-tabpanel");
      panel.id = item.panelId || `${item.id}-panel`;
      panel.setAttribute("role", "tabpanel");
      panel.setAttribute("aria-labelledby", tab.id);
      panel.hidden = !item.selected;
      append(panel, item.content || []);
      panels.push(panel);
    }
    function activate(index, focus = false) {
      buttons.forEach((tab, current) => {
        const selected = current === index;
        tab.setAttribute("aria-selected", String(selected));
        tab.tabIndex = selected ? 0 : -1;
        panels[current].hidden = !selected;
      });
      if (focus && buttons[index] && !buttons[index].disabled) buttons[index].focus();
      if (typeof options.onChange === "function") options.onChange(index);
    }
    root.addEventListener("keydown", event => {
      if (!["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      const available = buttons.map((tab, index) => tab.disabled ? -1 : index).filter(index => index >= 0);
      if (!available.length) return;
      const current = buttons.findIndex(tab => tab.getAttribute("aria-selected") === "true");
      let target = available[0];
      if (event.key === "End") target = available[available.length - 1];
      else if (event.key === "ArrowLeft") target = available[(available.indexOf(current) - 1 + available.length) % available.length];
      else if (event.key === "ArrowRight") target = available[(available.indexOf(current) + 1) % available.length];
      activate(target, true);
    });
    const wrapper = node("div");
    wrapper.append(root, ...panels);
    wrapper.uiTabs = { root, panels, activate };
    return wrapper;
  }

  function breadcrumb(options = {}) {
    const root = node("nav", "ui-breadcrumb");
    setDataComponent(root, COMPONENTS.breadcrumb);
    root.setAttribute("aria-label", options.label || "Breadcrumb");
    for (const [index, item] of (options.items || []).entries()) {
      if (index) append(root, node("span", "ui-breadcrumb-separator", "/"));
      const isCurrent = index === options.items.length - 1;
      const element = item.href ? node("a", "", item.label) : node("span", "", item.label);
      if (item.href) element.href = item.href;
      if (isCurrent) element.setAttribute("aria-current", "page");
      append(root, element);
    }
    return root;
  }

  function toolbar(options = {}) {
    const root = node("div", "ui-toolbar");
    setDataComponent(root, COMPONENTS.toolbar);
    root.setAttribute("role", "toolbar");
    root.setAttribute("aria-label", options.label || "Toolbar");
    root.setAttribute("aria-orientation", options.orientation || "horizontal");
    for (const group of options.groups || []) {
      const container = node("div", "ui-toolbar-group");
      append(container, group);
      root.append(container);
    }
    return root;
  }

  function table(options = {}) {
    const root = node("div", "ui-table-wrap");
    setDataComponent(root, COMPONENTS.table);
    const element = node("table", "ui-table");
    if (options.caption) element.append(node("caption", "", options.caption));
    if (options.columns) {
      const head = node("thead");
      const row = node("tr");
      for (const column of options.columns) {
        const cell = node("th", "", column.label || column.key);
        cell.scope = "col";
        row.append(cell);
      }
      head.append(row);
      element.append(head);
    }
    const body = node("tbody");
    for (const rowData of options.rows || []) {
      const row = node("tr");
      const cells = Array.isArray(rowData) ? rowData : (options.columns || []).map(column => rowData[column.key]);
      for (const value of cells) {
        const cell = node("td");
        append(cell, value instanceof Node ? value : String(value ?? ""));
        row.append(cell);
      }
      body.append(row);
    }
    element.append(body);
    root.append(element);
    return root;
  }

  function virtualList(options = {}) {
    const root = node("div", "ui-virtual-list");
    setDataComponent(root, COMPONENTS.virtualList);
    root.setAttribute("role", "listbox");
    if (options.label) root.setAttribute("aria-label", options.label);
    root.tabIndex = 0;
    const spacer = node("div", "ui-virtual-list-spacer");
    const windowElement = node("div", "ui-virtual-list-window");
    spacer.append(windowElement);
    root.append(spacer);
    let items = [];
    let itemHeight = Math.max(24, Number(options.itemHeight) || 40);
    let renderItem = options.renderItem || ((item) => item);
    let selectedIndex = -1;
    let overscan = Math.max(1, Number(options.overscan) || 3);

    function render() {
      const count = items.length;
      root.setAttribute("aria-setsize", String(count));
      spacer.style.height = `${count * itemHeight}px`;
      const start = Math.max(0, Math.floor(root.scrollTop / itemHeight) - overscan);
      const visible = Math.ceil((root.clientHeight || itemHeight * 10) / itemHeight) + overscan * 2;
      const end = Math.min(count, start + visible);
      windowElement.style.transform = `translateY(${start * itemHeight}px)`;
      windowElement.replaceChildren();
      for (let index = start; index < end; index += 1) {
        const item = node("div", "ui-virtual-list-item");
        item.setAttribute("role", "option");
        item.setAttribute("aria-posinset", String(index + 1));
        item.setAttribute("aria-setsize", String(count));
        item.setAttribute("aria-selected", String(index === selectedIndex));
        item.style.height = `${itemHeight}px`;
        append(item, renderItem(items[index], index));
        item.addEventListener("click", () => {
          selectedIndex = index;
          render();
          if (typeof options.onSelect === "function") options.onSelect(items[index], index);
        });
        windowElement.append(item);
      }
    }

    root.addEventListener("scroll", render, { passive: true });
    root.addEventListener("keydown", event => {
      if (!items.length || !["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      event.preventDefault();
      if (event.key === "Home") selectedIndex = 0;
      else if (event.key === "End") selectedIndex = items.length - 1;
      else selectedIndex = Math.min(items.length - 1, Math.max(0, selectedIndex + (event.key === "ArrowDown" ? 1 : -1)));
      const top = selectedIndex * itemHeight;
      if (top < root.scrollTop) root.scrollTop = top;
      if (top + itemHeight > root.scrollTop + root.clientHeight) root.scrollTop = top + itemHeight - root.clientHeight;
      render();
      if (typeof options.onSelect === "function") options.onSelect(items[selectedIndex], selectedIndex);
    });
    root.uiVirtualList = {
      setItems(nextItems) {
        items = Array.from(nextItems || []);
        selectedIndex = Math.min(selectedIndex, items.length - 1);
        render();
      },
      setItemHeight(value) {
        itemHeight = Math.max(24, Number(value) || itemHeight);
        render();
      },
      refresh: render
    };
    requestAnimationFrame(render);
    return root;
  }

  function badge(label, options = {}) {
    const element = node("span", "ui-badge", label);
    setDataComponent(element, COMPONENTS.badge);
    element.dataset.tone = tone(options.tone);
    if (options.title) element.title = options.title;
    return element;
  }

  function statusBadge(label, options = {}) {
    const element = node("span", "ui-status-badge", label);
    setDataComponent(element, COMPONENTS.statusBadge);
    element.dataset.tone = tone(options.tone || "neutral");
    if (options.live) {
      element.setAttribute("role", "status");
      element.setAttribute("aria-live", options.assertive ? "assertive" : "polite");
    }
    return element;
  }

  function capabilityRing(active = [], options = {}) {
    const root = node("div", "ui-capability-ring");
    setDataComponent(root, COMPONENTS.capabilityRing);
    if (options.label) root.setAttribute("aria-label", options.label);
    const svg = svgNode("svg", { viewBox: "0 0 36 36", "aria-hidden": "true" });
    const names = ["coordinator", "relay", "exit", "file"];
    const circumference = 2 * Math.PI * 14;
    const segment = circumference / names.length;
    names.forEach((name, index) => {
      const circle = svgNode("circle", {
        class: "ui-capability-ring-segment",
        cx: 18,
        cy: 18,
        r: 14,
        "data-capability": name,
        "data-active": String(active.includes(name)),
        "stroke-dasharray": `${Math.max(1, segment - 3)} ${circumference}`,
        "stroke-dashoffset": -index * segment,
        transform: "rotate(-90 18 18)"
      });
      svg.append(circle);
    });
    const legend = node("div", "ui-capability-ring-legend");
    for (const name of names) {
      const item = node("span", "", options.labels?.[name] || name);
      item.dataset.capability = name;
      item.dataset.active = String(active.includes(name));
      legend.append(item);
    }
    root.append(svg, legend);
    return root;
  }

  function dialog(options = {}) {
    const root = node("div", "ui-dialog-backdrop");
    setDataComponent(root, COMPONENTS.dialog);
    root.hidden = true;
    const panel = node("section", "ui-dialog");
    panel.setAttribute("role", "dialog");
    panel.setAttribute("aria-modal", "true");
    const header = node("header", "ui-dialog-header");
    const heading = node("div");
    const title = node("h2", "ui-dialog-title", options.title || "Dialog");
    title.id = options.titleId || `ui-dialog-title-${dialogs.size + 1}`;
    panel.setAttribute("aria-labelledby", title.id);
    heading.append(title);
    if (options.description) heading.append(node("p", "ui-dialog-description", options.description));
    const close = button({ label: "关闭", iconOnly: true, variant: "ghost", ariaLabel: "关闭对话框", onClick: () => closeDialog(root) });
    close.textContent = "×";
    header.append(heading, close);
    const body = node("div", "ui-dialog-body");
    append(body, options.content || []);
    const actions = node("footer", "ui-dialog-actions");
    append(actions, options.actions || []);
    panel.append(header, body, actions);
    root.append(panel);

    root.addEventListener("mousedown", event => {
      if (event.target === root && options.dismissible !== false) closeDialog(root);
    });
    root.addEventListener("keydown", event => {
      if (event.key === "Escape" && options.dismissible !== false) {
        event.preventDefault();
        closeDialog(root);
      }
      if (event.key === "Tab") trapFocus(root, event);
    });
    root._uiOptions = options;
    dialogs.add(root);
    return root;
  }

  function trapFocus(root, event) {
    const focusable = [...root.querySelectorAll(FOCUSABLE)].filter(element => !element.hidden);
    if (!focusable.length) {
      event.preventDefault();
      return;
    }
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  function openDialog(target) {
    const root = typeof target === "string" ? document.querySelector(target) : target;
    if (!root) return null;
    dialogReturnFocus = document.activeElement;
    root.hidden = false;
    root.dataset.open = "true";
    activeDialog = root;
    const focusable = root.querySelector(FOCUSABLE);
    requestAnimationFrame(() => (focusable || root.querySelector(".ui-dialog"))?.focus());
    return root;
  }

  function closeDialog(target = activeDialog) {
    const root = typeof target === "string" ? document.querySelector(target) : target;
    if (!root) return;
    root.hidden = true;
    delete root.dataset.open;
    if (activeDialog === root) activeDialog = null;
    if (dialogReturnFocus && typeof dialogReturnFocus.focus === "function") dialogReturnFocus.focus();
    dialogReturnFocus = null;
  }

  function menu(options = {}) {
    const root = node("div", "ui-menu");
    setDataComponent(root, COMPONENTS.menu);
    root.setAttribute("role", "menu");
    root.hidden = true;
    if (options.label) root.setAttribute("aria-label", options.label);
    populateMenu(root, options);
    openMenus.add(root);
    return root;
  }

  function contextMenu(options = {}) {
    const root = node("div", "ui-context-menu");
    setDataComponent(root, COMPONENTS.contextMenu);
    root.setAttribute("role", "menu");
    root.hidden = true;
    if (options.label) root.setAttribute("aria-label", options.label);
    populateMenu(root, options);
    openMenus.add(root);
    return root;
  }

  function populateMenu(root, options) {
    root.replaceChildren();
    if (options.title) root.append(node("div", "ui-menu-header", options.title));
    for (const item of options.items || []) {
      if (item.separator) {
        const separator = node("div", "ui-menu-separator");
        separator.setAttribute("role", "separator");
        root.append(separator);
        continue;
      }
      const action = node("button", "ui-menu-item", item.label || "");
      action.type = "button";
      action.setAttribute("role", "menuitem");
      if (item.danger) action.dataset.variant = "danger";
      if (item.disabled) action.disabled = true;
      if (item.shortcut) {
        const shortcut = node("span", "", item.shortcut);
        shortcut.style.marginLeft = "auto";
        action.append(shortcut);
      }
      action.addEventListener("click", () => {
        hideAllMenus();
        if (typeof item.onSelect === "function") item.onSelect();
      });
      root.append(action);
    }
    root.addEventListener("keydown", event => {
      if (event.key === "Escape") {
        hideAllMenus();
        return;
      }
      if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
      const items = [...root.querySelectorAll(".ui-menu-item:not(:disabled)")];
      if (!items.length) return;
      event.preventDefault();
      const current = items.indexOf(document.activeElement);
      let target = 0;
      if (event.key === "End") target = items.length - 1;
      else if (event.key === "ArrowUp") target = (current - 1 + items.length) % items.length;
      else if (event.key === "ArrowDown") target = (current + 1) % items.length;
      items[target].focus();
    });
  }

  function showMenu(target, x, y) {
    const root = typeof target === "string" ? document.querySelector(target) : target;
    if (!root) return;
    hideAllMenus(false);
    root.hidden = false;
    root.style.left = "0px";
    root.style.top = "0px";
    const rect = root.getBoundingClientRect();
    const left = Math.max(8, Math.min(Number(x) || 0, window.innerWidth - rect.width - 8));
    const top = Math.max(8, Math.min(Number(y) || 0, window.innerHeight - rect.height - 8));
    root.style.left = `${left}px`;
    root.style.top = `${top}px`;
    const first = root.querySelector(".ui-menu-item:not(:disabled)");
    requestAnimationFrame(() => first?.focus());
  }

  function hideAllMenus(focus = true) {
    for (const menuElement of openMenus) {
      if (menuElement.hidden) continue;
      menuElement.hidden = true;
      if (focus && menuElement.contains(document.activeElement)) document.activeElement.blur();
    }
  }

  function tooltip(target, options = {}) {
    const owner = typeof target === "string" ? document.querySelector(target) : target;
    if (!owner) return null;
    const id = `ui-tooltip-${++tooltipSequence}`;
    let element = null;
    function show() {
      if (!element) {
        element = node("div", "ui-tooltip", options.text || "");
        setDataComponent(element, COMPONENTS.tooltip);
        element.id = id;
        element.setAttribute("role", "tooltip");
        document.body.append(element);
      }
      const rect = owner.getBoundingClientRect();
      element.hidden = false;
      element.style.left = `${Math.min(window.innerWidth - element.offsetWidth - 8, Math.max(8, rect.left))}px`;
      element.style.top = `${Math.max(8, rect.top - element.offsetHeight - 8)}px`;
      owner.setAttribute("aria-describedby", id);
    }
    function hide() {
      if (element) element.hidden = true;
      owner.removeAttribute("aria-describedby");
    }
    owner.addEventListener("mouseenter", show);
    owner.addEventListener("mouseleave", hide);
    owner.addEventListener("focus", show);
    owner.addEventListener("blur", hide);
    owner.addEventListener("keydown", event => {
      if (event.key === "Escape") hide();
    });
    return owner;
  }

  function toastRegion() {
    let region = document.querySelector(".ui-toast-region");
    if (!region) {
      region = node("div", "ui-toast-region");
      region.setAttribute("role", "region");
      region.setAttribute("aria-label", "通知");
      document.body.append(region);
    }
    return region;
  }

  function toast(message, options = {}) {
    const region = toastRegion();
    const element = node("div", "ui-toast");
    setDataComponent(element, COMPONENTS.toast);
    element.dataset.tone = tone(options.tone || "info");
    element.setAttribute("role", options.assertive ? "alert" : "status");
    element.setAttribute("aria-live", options.assertive ? "assertive" : "polite");
    element.dataset.toastId = String(++toastSequence);
    const text = node("div", "ui-toast-message", message);
    const dismiss = node("button", "ui-toast-dismiss", "×");
    dismiss.type = "button";
    dismiss.setAttribute("aria-label", "关闭通知");
    dismiss.addEventListener("click", () => element.remove());
    element.append(text, dismiss);
    region.append(element);
    const duration = Number.isFinite(options.duration) ? options.duration : 2600;
    if (duration > 0) window.setTimeout(() => element.remove(), duration);
    return element;
  }

  function state(component, options = {}) {
    const variant = component.endsWith("-state") ? component.slice(0, -6) : component;
    const root = node("section", `ui-state ui-state-${variant}`);
    setDataComponent(root, component);
    const icon = node("div", "ui-state-icon");
    append(icon, options.icon || []);
    const title = node("h2", "ui-state-title", options.title || "");
    const description = node("p", "ui-state-description", options.description || "");
    root.append(icon, title, description);
    if (options.actions?.length) {
      const actions = node("div", "ui-state-actions");
      append(actions, options.actions);
      root.append(actions);
    }
    return root;
  }

  function emptyState(options = {}) {
    return state(COMPONENTS.emptyState, options);
  }

  function errorState(options = {}) {
    const root = state(COMPONENTS.errorState, options);
    root.setAttribute("role", "alert");
    if (options.diagnosticId) root.dataset.diagnosticId = options.diagnosticId;
    return root;
  }

  function permissionDeniedState(options = {}) {
    return state(COMPONENTS.permissionDeniedState, options);
  }

  function unsupportedState(options = {}) {
    return state(COMPONENTS.unsupportedState, options);
  }

  function init(root = document) {
    for (const element of root.querySelectorAll("[data-component]")) {
      const component = element.dataset.component;
      if (component === "dialog") {
        element.hidden = element.hidden && !element.classList.contains("active");
        element.querySelector(".ui-dialog")?.setAttribute("aria-modal", "true");
      }
      if (component === "toolbar") element.setAttribute("role", "toolbar");
      if (component === "table") element.querySelector("table")?.setAttribute("role", "table");
    }
  }

  document.addEventListener("pointerdown", event => {
    for (const menuElement of openMenus) {
      if (!menuElement.hidden && !menuElement.contains(event.target)) menuElement.hidden = true;
    }
  });
  document.addEventListener("keydown", event => {
    if (event.key === "Escape") hideAllMenus();
  });

  const api = Object.freeze({
    components: COMPONENTS,
    button,
    input,
    select,
    checkbox,
    radio,
    tabs,
    breadcrumb,
    toolbar,
    table,
    virtualList,
    badge,
    statusBadge,
    capabilityRing,
    dialog,
    openDialog,
    closeDialog,
    menu,
    contextMenu,
    showMenu,
    hideMenu: hideAllMenus,
    tooltip,
    toast,
    emptyState,
    errorState,
    permissionDeniedState,
    unsupportedState,
    init
  });

  window.FuckIPv6UI = api;
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", () => init(), { once: true });
  } else {
    init();
  }
})();
