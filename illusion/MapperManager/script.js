/**
 * MapperManager - Kindle Button Mapper config UI
 * ES5 compatible - WebKit 533
 * Talks to the --waf-helper HTTP server on localhost:8322.
 */

var MapperManager = (function() {
    "use strict";

    var HELPER_URL = "http://localhost:8322";
    var MESSAGE_TIMEOUT = 4000;

    var messageTimer = null;
    var actions = [];          // [{id, label}] from /actions
    var layouts = [];          // [{code, name}] from /layouts
    var devices = [];          // [{path, name, uniq}]
    var ini = null;            // parsed config
    var pendingSlot = null;    // slot object awaiting an action pick
    var captureXhrAbort = null;
    var actionTab = "koreader"; // which tab is showing in the action picker
    var currentDeviceId = null; // currently selected device on Bindings tab
    var editingDeviceId = null; // device being edited in the detail overlay (null = new)
    var continuousCapture = false; // when true, re-fire capture after each action pick
    var deviceScanTimer = null; // Device tab auto-refresh interval
    var lastDevicesSig = "";    // last /devices result, to skip redundant renders

    var DEVICE_KINDS = ["buttons", "longpress", "dpad", "dpad_longpress", "triggers", "triggers_longpress"];

    var _kbdKeys = null;
    function buildKeyboardKeys() {
        var arr = [];
        var i;
        function add(id, label) { arr.push({ id: id, label: label }); }

        // Letters & digits
        for (i = 0; i < 26; i++) {
            var ch = String.fromCharCode(65 + i);
            add("KEY_" + ch, ch);
        }
        for (i = 0; i <= 9; i++) add("KEY_" + i, String(i));

        // Function keys
        for (i = 1; i <= 24; i++) add("KEY_F" + i, "F" + i);

        // Navigation
        add("KEY_UP", "Up"); add("KEY_DOWN", "Down");
        add("KEY_LEFT", "Left"); add("KEY_RIGHT", "Right");
        add("KEY_HOME", "Home"); add("KEY_END", "End");
        add("KEY_PAGEUP", "Page Up"); add("KEY_PAGEDOWN", "Page Down");
        add("KEY_INSERT", "Insert"); add("KEY_DELETE", "Delete");
        add("KEY_BACKSPACE", "Backspace");

        // Editing / whitespace
        add("KEY_ENTER", "Enter"); add("KEY_SPACE", "Space");
        add("KEY_TAB", "Tab"); add("KEY_ESC", "Escape");

        // Modifiers
        add("KEY_LEFTSHIFT", "Left Shift"); add("KEY_RIGHTSHIFT", "Right Shift");
        add("KEY_LEFTCTRL", "Left Ctrl"); add("KEY_RIGHTCTRL", "Right Ctrl");
        add("KEY_LEFTALT", "Left Alt"); add("KEY_RIGHTALT", "Right Alt");
        add("KEY_LEFTMETA", "Left Meta"); add("KEY_RIGHTMETA", "Right Meta");
        add("KEY_CAPSLOCK", "Caps Lock"); add("KEY_NUMLOCK", "Num Lock");
        add("KEY_SCROLLLOCK", "Scroll Lock");

        // Punctuation & symbols
        add("KEY_MINUS", "-"); add("KEY_EQUAL", "=");
        add("KEY_LEFTBRACE", "["); add("KEY_RIGHTBRACE", "]");
        add("KEY_SEMICOLON", ";"); add("KEY_APOSTROPHE", "'");
        add("KEY_GRAVE", "`"); add("KEY_BACKSLASH", "\\");
        add("KEY_COMMA", ","); add("KEY_DOT", ".");
        add("KEY_SLASH", "/");

        // Numpad
        for (i = 0; i <= 9; i++) add("KEY_KP" + i, "Numpad " + i);
        add("KEY_KPPLUS", "Numpad +"); add("KEY_KPMINUS", "Numpad -");
        add("KEY_KPASTERISK", "Numpad *"); add("KEY_KPSLASH", "Numpad /");
        add("KEY_KPDOT", "Numpad ."); add("KEY_KPENTER", "Numpad Enter");
        add("KEY_KPEQUAL", "Numpad =");

        // Media
        add("KEY_VOLUMEUP", "Volume +"); add("KEY_VOLUMEDOWN", "Volume -");
        add("KEY_MUTE", "Mute");
        add("KEY_PLAYPAUSE", "Play/Pause"); add("KEY_PLAY", "Play");
        add("KEY_PAUSE", "Pause"); add("KEY_STOP", "Stop");
        add("KEY_NEXTSONG", "Next Track"); add("KEY_PREVIOUSSONG", "Prev Track");
        add("KEY_FORWARD", "Forward"); add("KEY_REWIND", "Rewind");
        add("KEY_RECORD", "Record"); add("KEY_EJECTCD", "Eject");

        // Power / system
        add("KEY_POWER", "Power"); add("KEY_SLEEP", "Sleep");
        add("KEY_WAKEUP", "Wake Up"); add("KEY_SUSPEND", "Suspend");
        add("KEY_MENU", "Menu"); add("KEY_BACK", "Back");
        add("KEY_HOMEPAGE", "Homepage");

        // Screen / brightness
        add("KEY_BRIGHTNESSUP", "Brightness +");
        add("KEY_BRIGHTNESSDOWN", "Brightness -");
        add("KEY_DISPLAYTOGGLE", "Display Toggle");

        // Browser
        add("KEY_REFRESH", "Refresh"); add("KEY_SEARCH", "Search");
        add("KEY_BOOKMARKS", "Bookmarks");

        // Gamepad-ish that show up as keys on some HID descriptors
        add("KEY_PRINT", "Print"); add("KEY_SYSRQ", "SysRq");
        add("KEY_CALC", "Calculator"); add("KEY_MAIL", "Mail");

        return arr;
    }

    function keyboardKeys() {
        if (!_kbdKeys) _kbdKeys = buildKeyboardKeys();
        return _kbdKeys;
    }

    var filteredKeys = null; // populated lazily by openActionPicker / search

    // ---- XHR ----

    function request(method, path, body, callback) {
        var xhr = new XMLHttpRequest();
        var url = HELPER_URL + path;
        var done = false;
        var timer = setTimeout(function() {
            if (!done) { xhr.abort(); callback(null, "timeout"); }
        }, 30000);

        xhr.onreadystatechange = function() {
            if (xhr.readyState !== 4) return;
            done = true;
            clearTimeout(timer);
            if (xhr.status === 200) {
                callback(xhr.responseText, null);
            } else if (xhr.status === 0) {
                callback(null, "connection failed");
            } else {
                callback(null, "HTTP " + xhr.status);
            }
        };
        try {
            xhr.open(method, url, true);
            if (body !== null && body !== undefined) {
                xhr.setRequestHeader("Content-Type", "text/plain");
                xhr.send(body);
            } else {
                xhr.send(null);
            }
        } catch (e) {
            done = true;
            clearTimeout(timer);
            callback(null, "send failed");
        }
        return xhr;
    }

    function getJSON(path, cb) {
        request("GET", path, null, function(text, err) {
            if (err) return cb(null, err);
            try { cb(JSON.parse(text), null); }
            catch (e) { cb(null, "bad JSON"); }
        });
    }

    function postJSON(path, body, cb) {
        request("POST", path, body, function(text, err) {
            if (err) return cb(null, err);
            try { cb(JSON.parse(text), null); }
            catch (e) { cb(null, "bad JSON"); }
        });
    }

    // ---- UI helpers ----

    function getEl(id) { return document.getElementById(id); }

    function escapeHtml(str) {
        if (str === null || str === undefined) return "";
        return String(str).replace(/&/g, "&amp;")
            .replace(/</g, "&lt;")
            .replace(/>/g, "&gt;")
            .replace(/"/g, "&quot;");
    }

    function showMessage(text, isError) {
        var bar = getEl("messageBar");
        bar.innerHTML = escapeHtml(text);
        bar.className = "message-bar visible" + (isError ? " error" : "");
        if (messageTimer) clearTimeout(messageTimer);
        messageTimer = setTimeout(function() {
            bar.className = "message-bar";
        }, MESSAGE_TIMEOUT);
    }

    function showOverlay(id) { getEl(id).className = "overlay visible"; }
    function hideOverlay(id) { getEl(id).className = "overlay"; }

    function showInfo(text) {
        getEl("infoMessage").innerHTML = escapeHtml(text);
        showOverlay("infoOverlay");
    }

    // ---- INI parsing (minimal) ----

    // Returns { sections: { name: [ [key, value], ... ] }, order: [...] }
    function parseIni(text) {
        var sections = {};
        var order = [];
        var current = "_global";
        sections[current] = [];
        order.push(current);

        var lines = text.split(/\r?\n/);
        for (var i = 0; i < lines.length; i++) {
            var line = lines[i];
            var trimmed = line.replace(/^\s+|\s+$/g, "");
            if (!trimmed || trimmed.charAt(0) === "#" || trimmed.charAt(0) === ";") continue;
            if (trimmed.charAt(0) === "[" && trimmed.charAt(trimmed.length - 1) === "]") {
                current = trimmed.substring(1, trimmed.length - 1);
                if (!sections[current]) {
                    sections[current] = [];
                    order.push(current);
                }
                continue;
            }
            var eq = trimmed.indexOf("=");
            if (eq < 0) continue;
            var k = trimmed.substring(0, eq).replace(/\s+$/, "");
            var v = trimmed.substring(eq + 1).replace(/^\s+/, "");
            sections[current].push([k, v]);
        }
        return { sections: sections, order: order };
    }

    function serializeIni(parsed) {
        var out = [];
        for (var i = 0; i < parsed.order.length; i++) {
            var name = parsed.order[i];
            var entries = parsed.sections[name];
            if (name !== "_global") {
                out.push("[" + name + "]");
            }
            for (var j = 0; j < entries.length; j++) {
                out.push(entries[j][0] + " = " + entries[j][1]);
            }
            out.push("");
        }
        return out.join("\n");
    }

    function getValue(section, key) {
        var entries = ini.sections[section];
        if (!entries) return null;
        for (var i = 0; i < entries.length; i++) {
            if (entries[i][0] === key) return entries[i][1];
        }
        return null;
    }

    function setValue(section, key, value) {
        if (!ini.sections[section]) {
            ini.sections[section] = [];
            ini.order.push(section);
        }
        var entries = ini.sections[section];
        for (var i = 0; i < entries.length; i++) {
            if (entries[i][0] === key) { entries[i][1] = value; return; }
        }
        entries.push([key, value]);
    }

    function delValue(section, key) {
        var entries = ini.sections[section];
        if (!entries) return;
        for (var i = 0; i < entries.length; i++) {
            if (entries[i][0] === key) { entries.splice(i, 1); return; }
        }
    }

    function delSection(name) {
        if (!ini.sections[name]) return;
        delete ini.sections[name];
        var idx = ini.order.indexOf(name);
        if (idx >= 0) ini.order.splice(idx, 1);
    }

    function renameSection(oldName, newName) {
        if (!ini.sections[oldName]) return;
        ini.sections[newName] = ini.sections[oldName];
        delete ini.sections[oldName];
        for (var i = 0; i < ini.order.length; i++) {
            if (ini.order[i] === oldName) { ini.order[i] = newName; return; }
        }
    }

    function listDeviceIds() {
        var ids = [];
        for (var i = 0; i < ini.order.length; i++) {
            var name = ini.order[i];
            var parts = name.split(".");
            if (parts.length === 2 && parts[0] === "device") {
                if (ids.indexOf(parts[1]) < 0) ids.push(parts[1]);
            }
        }
        return ids;
    }

    function migrateLegacy() {
        if (listDeviceIds().length > 0) return;
        if (!ini.sections.device) return;

        renameSection("device", "device.default");
        for (var i = 0; i < DEVICE_KINDS.length; i++) {
            var k = DEVICE_KINDS[i];
            if (ini.sections[k]) renameSection(k, "device.default." + k);
        }
    }

    // ---- Tabs ----

    function bindTabs() {
        var tabs = document.querySelectorAll(".tab");
        for (var i = 0; i < tabs.length; i++) {
            tabs[i].addEventListener("click", onTabClick, false);
        }
    }

    function onTabClick(e) {
        var name = e.currentTarget.getAttribute("data-tab");
        var tabs = document.querySelectorAll(".tab");
        for (var i = 0; i < tabs.length; i++) {
            tabs[i].className = "tab" + (tabs[i].getAttribute("data-tab") === name ? " tab-active" : "");
        }
        var panes = document.querySelectorAll(".tab-content");
        for (var j = 0; j < panes.length; j++) {
            panes[j].className = "tab-content" + (panes[j].id === "tab-" + name ? " tab-visible" : "");
        }
        if (name === "device") { refreshDevices(); refreshStatus(); startDeviceAutoRefresh(); }
        else { stopDeviceAutoRefresh(); }
        if (name === "debug") { renderRawConfig(); }
    }

    // ---- Bindings render ----

    function devSection(kind) {
        if (!currentDeviceId) return null;
        return "device." + currentDeviceId + "." + kind;
    }

    function renderBindingsPicker() {
        var container = getEl("bindingsDevicePills");
        var ids = listDeviceIds();
        if (ids.length === 0) {
            container.innerHTML = '<button class="device-pill-add" data-action="new">+ Add device</button>';
            currentDeviceId = null;
            return;
        }
        if (!currentDeviceId || ids.indexOf(currentDeviceId) < 0) {
            currentDeviceId = ids[0];
        }
        var html = "";
        for (var i = 0; i < ids.length; i++) {
            var id = ids[i];
            var name = getValue("device." + id, "name") || id;
            var cls = "device-pill" + (id === currentDeviceId ? " pill-active" : "");
            html += '<button class="' + cls + '" data-dev-id="' + escapeHtml(id) + '">' + escapeHtml(name) + "</button>";
        }
        html += '<button class="device-pill-add" data-action="new">+</button>';
        container.innerHTML = html;
    }

    function renderBindings() {
        renderBindingsPicker();

        var rows = [];
        if (currentDeviceId) {
            var sectionList = [
                { kind: "buttons",            label: "Btn" },
                { kind: "longpress",          label: "Btn Long" },
                { kind: "dpad",               label: "DPad" },
                { kind: "dpad_longpress",     label: "DPad Long" },
                { kind: "triggers",           label: "Trigger" },
                { kind: "triggers_longpress", label: "Trig Long" }
            ];

            for (var s = 0; s < sectionList.length; s++) {
                var sec = sectionList[s];
                var sectionName = "device." + currentDeviceId + "." + sec.kind;
                var entries = ini.sections[sectionName] || [];
                for (var i = 0; i < entries.length; i++) {
                    rows.push(renderBindingRow(sectionName, sec.label, entries[i][0], entries[i][1]));
                }
            }
        }

        var list = getEl("bindingsList");
        if (!currentDeviceId) {
            list.innerHTML = '<div class="binding-empty">Add a device first (Device tab)</div>';
        } else if (rows.length === 0) {
            list.innerHTML = '<div class="binding-empty">No mappings yet — tap + Add</div>';
        } else {
            list.innerHTML = rows.join("");
        }
    }

    function renderBindingRow(section, label, key, script) {
        var parsed = extractAction(script);
        var actionLabel = labelForAction(parsed) || parsed.id || script;
        return '<div class="binding-row" data-section="' + escapeHtml(section) + '" data-key="' + escapeHtml(key) + '">'
            + '<span class="binding-slot">' + escapeHtml(label) + " " + escapeHtml(key) + '</span>'
            + '<span class="binding-action">' + escapeHtml(actionLabel) + '</span>'
            + '<button class="binding-del" data-section="' + escapeHtml(section) + '" data-key="' + escapeHtml(key) + '">&#x2715;</button>'
            + '</div>';
    }

    function extractAction(script) {
        if (!script) return { kind: "other", id: "" };
        var m = script.match(/koreader\.sh\s+(.+?)\s*$/);
        if (m) return { kind: "koreader", id: m[1].replace(/^\s+|\s+$/g, "") };
        m = script.match(/key\.sh\s+(.+?)\s*$/);
        if (m) return { kind: "keyboard", id: m[1].replace(/^\s+|\s+$/g, "") };
        return { kind: "other", id: script };
    }

    function labelForAction(parsed) {
        var list = parsed.kind === "keyboard" ? keyboardKeys() : actions;
        for (var i = 0; i < list.length; i++) {
            if (list[i].id === parsed.id) {
                return (parsed.kind === "keyboard" ? "Key: " : "") + list[i].label;
            }
        }
        return null;
    }

    function scriptForAction(actionId, kind) {
        if (kind === "keyboard") {
            return "/mnt/us/kindle-button-mapper/scripts/key.sh " + actionId;
        }
        if (kind === "custom") {
            return actionId;
        }
        return "/mnt/us/kindle-button-mapper/scripts/koreader.sh " + actionId;
    }

    // ---- Slot picker (Add) ----

    function openAddPicker() { showOverlay("addOverlay"); }
    function closeAddPicker() { hideOverlay("addOverlay"); }

    function onAddOpt(e) {
        closeAddPicker();
        if (!currentDeviceId) {
            showMessage("Add a device first (Device tab)", true);
            return;
        }
        var kind = e.currentTarget.getAttribute("data-slot-kind");
        if (kind === "button") {
            captureNewButton(false);
            return;
        }
        var slot = mapKindToSlot(kind);
        if (!slot) return;
        pendingSlot = { section: devSection(slot.kind), key: slot.key };
        openActionPicker();
    }

    function mapKindToSlot(kind) {
        switch (kind) {
            case "dpad-up":     return { kind: "dpad",     key: "up" };
            case "dpad-down":   return { kind: "dpad",     key: "down" };
            case "dpad-left":   return { kind: "dpad",     key: "left" };
            case "dpad-right":  return { kind: "dpad",     key: "right" };
            case "trigger-lt":  return { kind: "triggers", key: "lt" };
            case "trigger-rt":  return { kind: "triggers", key: "rt" };
        }
        return null;
    }

    // ---- Capture flow ----

    function captureNewButton(longPress) {
        if (!currentDeviceId) { showMessage("Add a device first", true); return; }
        showMessage("Finding device...", false);
        resolveDevicePath(currentDeviceId, function(devPath, err) {
            if (!devPath) { showMessage(err || "Device not connected", true); return; }
            startCapture(devPath, longPress);
        });
    }

    function resolveDevicePath(id, cb) {
        var uniq = getValue("device." + id, "uniq") || "";
        var name = getValue("device." + id, "name") || "";
        getJSON("/devices", function(data, err) {
            if (err) { cb(null, "Devices: " + err); return; }
            var list = data.devices || [];
            var match = null;
            for (var i = 0; i < list.length; i++) {
                if (uniq && list[i].uniq === uniq) {
                    match = list[i];
                    break;
                }
            }
            if (!match && name) {
                for (var j = 0; j < list.length; j++) {
                    if (list[j].name === name) {
                        match = list[j];
                        break;
                    }
                }
            }
            if (match) {
                cb(match.path, null);
            } else {
                cb(null, "Device not connected");
            }
        });
    }

    function startCapture(devPath, longPress) {
        showOverlay("captureOverlay");
        getEl("captureMsg").innerHTML = continuousCapture
            ? "Press a button — Cancel to stop"
            : "Press a button now...";
        getEl("captureHint").innerHTML = "Listening on " + escapeHtml(devPath);

        var url = "/capture?device=" + encodeURIComponent(devPath) + "&timeout=8000";
        captureXhrAbort = request("GET", url, null, function(text, err) {
            captureXhrAbort = null;
            hideOverlay("captureOverlay");
            if (err) { showMessage("Capture: " + err, true); return; }
            var data;
            try { data = JSON.parse(text); } catch (e) { showMessage("bad JSON", true); return; }
            if (!data.ok) { showMessage(data.error || "capture failed", true); return; }

            // Map captured event to a (kind, key) within the current device.
            var slot = null;
            if (data.kind === "key") {
                slot = { kind: longPress ? "longpress" : "buttons", key: String(data.code) };
            } else if (data.kind === "dpad") {
                var dir = (data.code === 16)
                    ? (data.value > 0 ? "right" : "left")
                    : (data.value > 0 ? "down" : "up");
                slot = { kind: "dpad", key: dir };
            } else if (data.kind === "trigger") {
                slot = { kind: "triggers", key: data.code === 10 ? "lt" : "rt" };
            }
            if (!slot) { showMessage("unknown event", true); return; }
            pendingSlot = { section: devSection(slot.kind), key: slot.key };
            openActionPicker();
        });
    }

    function cancelCapture() {
        if (captureXhrAbort) { captureXhrAbort.abort(); captureXhrAbort = null; }
        hideOverlay("captureOverlay");
        continuousCapture = false;
    }

    // ---- Action picker ----

    function openActionPicker() {
        var existing = pendingSlot ? getValue(pendingSlot.section, pendingSlot.key) : null;
        var parsed = extractAction(existing);
        actionTab = parsed.kind === "keyboard" ? "keyboard"
                  : parsed.kind === "other" && existing ? "custom"
                  : "koreader";
        filteredKeys = keyboardKeys();
        getEl("actionSearch").value = "";
        getEl("actionCustomCmd").value = (parsed.kind === "other" && existing) ? existing : "";
        getEl("actionTitle").innerHTML = "Assign to " + escapeHtml(pendingSlot.section) + " / " + escapeHtml(pendingSlot.key);
        renderActionTab();
        showOverlay("actionOverlay");
    }

    function renderActionTab() {
        var tabs = document.querySelectorAll(".action-tab");
        for (var i = 0; i < tabs.length; i++) {
            var name = tabs[i].getAttribute("data-action-tab");
            tabs[i].className = "action-tab" + (name === actionTab ? " action-tab-active" : "");
        }
        var isKbd = actionTab === "keyboard";
        var isCustom = actionTab === "custom";
        getEl("actionSearch").className = "action-search" + (isKbd ? " visible" : "");
        getEl("actionList").className = "action-list" + (isKbd ? " with-search" : "") + (isCustom ? " hidden" : "");
        getEl("actionCustom").className = "action-custom" + (isCustom ? " visible" : "");
        getEl("actionList").style.display = isCustom ? "none" : "block";

        if (actionTab === "koreader") refreshKoreaderStatus();
        else getEl("koreaderStatus").className = "koreader-status hidden";

        if (isCustom) return;

        var items = isKbd ? filteredKeys : actions;
        var html = "";
        for (var j = 0; j < items.length; j++) {
            html += '<div class="action-item" data-id="' + escapeHtml(items[j].id) + '">'
                + escapeHtml(items[j].label) + " <span class=\"action-id\">" + escapeHtml(items[j].id) + "</span></div>";
        }
        var list = getEl("actionList");
        list.innerHTML = html;
        list.scrollTop = 0;
    }

    function refreshKoreaderStatus() {
        var el = getEl("koreaderStatus");
        el.className = "koreader-status hidden";
        getJSON("/koreader/status", function(data, err) {
            if (actionTab !== "koreader" || !data || err || data.autostart) return;
            el.className = "koreader-status";
            el.innerHTML = "KOReader HTTP Inspector is off. In KOReader, open "
                + "<b>Tools → More Tools → HTTP Inspector → Auto start HTTP server</b> "
                + "to use these actions.";
        });
    }

    function onActionTabClick(e) {
        var name = e.currentTarget.getAttribute("data-action-tab");
        if (!name || name === actionTab) return;
        actionTab = name;
        renderActionTab();
    }

    function onActionSearchInput() {
        var q = (getEl("actionSearch").value || "").toLowerCase().replace(/^\s+|\s+$/g, "");
        if (!q) {
            filteredKeys = keyboardKeys();
        } else {
            var out = [];
            for (var i = 0; i < keyboardKeys().length; i++) {
                var k = keyboardKeys()[i];
                if (k.id.toLowerCase().indexOf(q) >= 0 || k.label.toLowerCase().indexOf(q) >= 0) {
                    out.push(k);
                }
            }
            filteredKeys = out;
        }
        if (actionTab === "keyboard") renderActionTab();
    }

    function onActionItemClick(e) {
        var target = e.target;
        if (!target || !target.className) return;
        if (target.className.indexOf("action-item") < 0) return;
        var id = target.getAttribute("data-id");
        if (!id || !pendingSlot) return;
        setValue(pendingSlot.section, pendingSlot.key, scriptForAction(id, actionTab));
        hideOverlay("actionOverlay");
        pendingSlot = null;
        renderBindings();
        showMessage("Mapping added (unsaved)", false);
        if (continuousCapture) {
            setTimeout(function() { captureNewButton(false); }, 250);
        }
    }

    // ---- Binding row delete / edit (event delegation) ----

    function onBindingsListClick(e) {
        var target = e.target;
        if (!target) return;

        // Delete button takes priority over row-tap.
        if (target.className && target.className.indexOf("binding-del") >= 0) {
            var dSection = target.getAttribute("data-section");
            var dKey = target.getAttribute("data-key");
            delValue(dSection, dKey);
            renderBindings();
            showMessage("Removed (unsaved)", false);
            return;
        }

        // Tap anywhere else on a row -> re-pick the action for that slot.
        var row = target;
        while (row && row !== document) {
            if (row.className && row.className.indexOf("binding-row") >= 0) {
                var section = row.getAttribute("data-section");
                var key = row.getAttribute("data-key");
                if (section && key) {
                    pendingSlot = { section: section, key: key };
                    openActionPicker();
                }
                return;
            }
            row = row.parentNode;
        }
    }

    // ---- Save / Reload ----

    function saveConfig(callback) {
        var text = serializeIni(ini);
        request("POST", "/config", text, function(resp, err) {
            if (err) { showMessage("Save failed: " + err, true); if (callback) callback(false); return; }
            showMessage("Saved", false);
            if (callback) callback(true);
        });
    }

    function saveAndApply() {
        saveConfig(function(ok) {
            if (!ok) return;
            postJSON("/reload", "", function(data, err) {
                if (err) { showMessage("Reload: " + err, true); return; }
                if (data.ok) showMessage("Daemon restarted", false);
                else showMessage(data.error || "reload failed", true);
            });
        });
    }

    function reloadConfig() {
        getJSON("/actions", function(data, err) {
            if (data && data.actions) actions = data.actions;
            request("GET", "/config", null, function(text, err2) {
                if (err2) { showMessage("Reload: " + err2, true); return; }
                ini = parseIni(text);
                migrateLegacy();
                currentDeviceId = null;
                renderBindings();
                renderRawConfig();
                renderConfiguredDevices();
                showMessage("Config reloaded", false);
            });
        });
    }

    // ---- Device tab ----

    function refreshDevices(quiet) {
        if (!quiet) {
            renderConfiguredDevices();
            showMessage("Scanning /dev/input...", false);
        }
        getJSON("/devices", function(data, err) {
            if (err) { if (!quiet) showMessage("Devices: " + err, true); return; }
            var list = data.devices || [];
            var sig = JSON.stringify(list);
            if (sig !== lastDevicesSig) {
                lastDevicesSig = sig;
                devices = list;
                renderAvailableDeviceList();
            }
            if (!quiet) showMessage("Found " + list.length + " device(s)", false);
        });
    }

    function startDeviceAutoRefresh() {
        stopDeviceAutoRefresh();
        deviceScanTimer = setInterval(function() { refreshDevices(true); }, 4000);
    }

    function stopDeviceAutoRefresh() {
        if (deviceScanTimer) {
            clearInterval(deviceScanTimer);
            deviceScanTimer = null;
        }
    }

    function renderConfiguredDevices() {
        var ids = listDeviceIds();
        var html = "";
        for (var i = 0; i < ids.length; i++) {
            var id = ids[i];
            var name = getValue("device." + id, "name") || id;
            var sub = getValue("device." + id, "uniq") || getValue("device." + id, "name") || "(no id)";
            html += '<div class="device-row" data-dev-id="' + escapeHtml(id) + '">'
                + '<div class="device-row-name">' + escapeHtml(name) + '</div>'
                + '<div class="device-row-path">' + escapeHtml(sub) + '</div>'
                + '</div>';
        }
        var el = getEl("configuredDevices");
        el.innerHTML = html || '<div class="binding-empty">No devices configured — tap + New</div>';
    }

    function renderAvailableDeviceList() {
        // /dev/input/event* nodes. Tapping one prefills a new-device dialog
        // with its name and MAC so the user can save it as a configured device.
        var html = "";
        for (var i = 0; i < devices.length; i++) {
            var d = devices[i];
            html += '<div class="device-row" data-avail-path="' + escapeHtml(d.path) + '" data-avail-name="' + escapeHtml(d.name || "") + '" data-avail-uniq="' + escapeHtml(d.uniq || "") + '">'
                + '<div class="device-row-name">' + escapeHtml(d.name || "(unnamed)") + '</div>'
                + '<div class="device-row-path">' + escapeHtml(d.path) + '</div>'
                + '</div>';
        }
        getEl("deviceList").innerHTML = html || '<div class="binding-empty">No input devices</div>';
    }

    function onConfiguredDeviceClick(e) {
        var row = e.target;
        while (row && row !== document) {
            if (row.className && row.className.indexOf("device-row") >= 0) {
                var id = row.getAttribute("data-dev-id");
                if (id) { openDeviceDetail(id); return; }
            }
            row = row.parentNode;
        }
    }

    function onAvailableDeviceClick(e) {
        var row = e.target;
        while (row && row !== document) {
            if (row.className && row.className.indexOf("device-row") >= 0) {
                var path = row.getAttribute("data-avail-path");
                var name = row.getAttribute("data-avail-name");
                var uniq = row.getAttribute("data-avail-uniq");
                if (path) { openDeviceDetailNew(name, uniq); return; }
            }
            row = row.parentNode;
        }
    }

    // ---- Device detail / add dialog ----

    function openDeviceDetailNew(prefillName, prefillUniq) {
        editingDeviceId = null;
        getEl("deviceDetailTitle").innerHTML = "New device";
        getEl("devDetailName").value = prefillName || "";
        getEl("devDetailUniq").value = prefillUniq || "";
        getEl("devDetailGrab").className = "toggle";
        setDeviceLayout("");
        getEl("btnDeviceDelete").style.display = "none";
        updateDeviceIdView();
        showOverlay("deviceDetailOverlay");
    }

    function openDeviceDetail(id) {
        editingDeviceId = id;
        getEl("deviceDetailTitle").innerHTML = "Edit device";
        getEl("devDetailName").value = getValue("device." + id, "name") || "";
        getEl("devDetailUniq").value = getValue("device." + id, "uniq") || "";
        var grab = (getValue("device." + id, "grab") || "").toLowerCase() === "true";
        getEl("devDetailGrab").className = "toggle" + (grab ? " on" : "");
        setDeviceLayout(getValue("device." + id, "keyboard_layout") || "");
        getEl("btnDeviceDelete").style.display = "block";
        getEl("devDetailIdView").innerHTML = escapeHtml(id);
        showOverlay("deviceDetailOverlay");
    }

    function updateDeviceIdView() {
        // For new devices, derive the id from the name on the fly.
        var name = getEl("devDetailName").value || "";
        getEl("devDetailIdView").innerHTML = escapeHtml(autoIdFromName(name) || "device");
    }

    function closeDeviceDetail() { hideOverlay("deviceDetailOverlay"); editingDeviceId = null; }

    function toggleDeviceDetailGrab() {
        var t = getEl("devDetailGrab");
        t.className = t.className.indexOf(" on") >= 0 ? "toggle" : "toggle on";
    }

    function autoIdFromName(s) {
        return s.toLowerCase().replace(/[^a-z0-9]+/g, "_").replace(/^_+|_+$/g, "").substring(0, 30) || "device";
    }

    function saveDeviceDetail() {
        var name = getEl("devDetailName").value || "";
        var newId = editingDeviceId || autoIdFromName(name);
        if (!newId.match(/^[a-zA-Z0-9_-]+$/)) {
            showMessage("Name needs at least one letter or digit", true);
            return;
        }
        var uniq = (getEl("devDetailUniq").value || "").replace(/^\s+|\s+$/g, "");
        if (!uniq && !name) {
            showMessage("Set a name or MAC", true);
            return;
        }
        var grab = getEl("devDetailGrab").className.indexOf(" on") >= 0 ? "true" : "false";

        if (!editingDeviceId && listDeviceIds().indexOf(newId) >= 0) {
            showMessage("A device with that name already exists", true);
            return;
        }

        var layout = getEl("devDetailLayout").getAttribute("data-code") || "";

        setValue("device." + newId, "name", name);
        setValue("device." + newId, "grab", grab);
        if (uniq) { setValue("device." + newId, "uniq", uniq); } else { delValue("device." + newId, "uniq"); }
        if (layout) { setValue("device." + newId, "keyboard_layout", layout); } else { delValue("device." + newId, "keyboard_layout"); }
        delValue("device." + newId, "path");

        if (!currentDeviceId) currentDeviceId = newId;
        closeDeviceDetail();
        renderConfiguredDevices();
        renderBindings();
        showMessage(editingDeviceId ? "Device updated (unsaved)" : "Device added (unsaved)", false);
    }

    function deleteDeviceDetail() {
        if (!editingDeviceId) { closeDeviceDetail(); return; }
        var id = editingDeviceId;
        delSection("device." + id);
        for (var i = 0; i < DEVICE_KINDS.length; i++) {
            delSection("device." + id + "." + DEVICE_KINDS[i]);
        }
        if (currentDeviceId === id) currentDeviceId = null;
        closeDeviceDetail();
        renderConfiguredDevices();
        renderBindings();
        showMessage("Device removed (unsaved)", false);
    }

    function refreshStatus() {
        getJSON("/status", function(data, err) {
            if (err) { getEl("daemonStatus").innerHTML = "unknown"; return; }
            getEl("daemonStatus").innerHTML = data.running
                ? ("running (pid " + data.pid + ")")
                : "not running";
            if (data.version) {
                var v = "v" + escapeHtml(data.version);
                if (data.build) v += " (" + escapeHtml(data.build) + ")";
                getEl("footerVersion").innerHTML = "Button Mapper " + v;
            }
        });
    }

    function restartDaemon() {
        postJSON("/reload", "", function(data, err) {
            if (err) { showMessage("Restart: " + err, true); return; }
            if (data.ok) { showMessage("Daemon restarted", false); refreshStatus(); }
            else showMessage(data.error || "failed", true);
        });
    }

    function stopDaemon() {
        postJSON("/stop", "", function(data, err) {
            if (err) { showMessage("Stop: " + err, true); return; }
            if (data.ok) { showMessage("Daemon stopped", false); refreshStatus(); }
            else showMessage(data.error || "failed", true);
        });
    }

    function startDaemon() {
        postJSON("/start", "", function(data, err) {
            if (err) { showMessage("Start: " + err, true); return; }
            if (data.ok) { showMessage("Daemon started", false); refreshStatus(); }
            else showMessage(data.error || "failed", true);
        });
    }

    // ---- Debug tab ----

    function renderRawConfig() {
        getEl("rawConfig").value = ini ? serializeIni(ini) : "";
    }

    function saveRawConfig() {
        var text = getEl("rawConfig").value;
        request("POST", "/config", text, function(resp, err) {
            if (err) { showMessage("Save: " + err, true); return; }
            ini = parseIni(text);
            renderBindings();
            showMessage("Raw config saved", false);
        });
    }

    function liveCapture() {
        captureNewButton(false);
    }

    function openLogs() {
        getEl("logsContent").innerHTML = "Loading...";
        showOverlay("logsOverlay");
        request("GET", "/logs", null, function(text, err) {
            if (err) { getEl("logsContent").innerHTML = escapeHtml("Error: " + err); return; }
            var lines = (text || "").split("\n");
            for (var i = 0; i < lines.length; i++) { lines[i] = formatLogLine(lines[i]); }
            getEl("logsContent").innerHTML = escapeHtml(lines.join("\n")) || "(empty)";
        });
    }

    function formatLogLine(line) {
        var m = line.match(/^\[\d{4}-\d{2}-\d{2}T(\d{2}:\d{2}:\d{2})Z\s+(\w+)\s+([^\]]*)\]\s?(.*)$/);
        if (!m) return line;
        var mod = m[3].replace(/^kindle_button_mapper(::)?/, "");
        return m[1] + " " + m[2] + (mod ? " " + mod : "") + ": " + m[4];
    }

    function closeLogs() { hideOverlay("logsOverlay"); }

    // ---- Init ----

    function bindEvents() {
        bindTabs();
        getEl("btnAdd").addEventListener("click", openAddPicker, false);
        getEl("btnAddCancel").addEventListener("click", closeAddPicker, false);
        getEl("btnSave").addEventListener("click", saveAndApply, false);
        getEl("btnReload").addEventListener("click", reloadConfig, false);
        getEl("btnRescan").addEventListener("click", refreshDevices, false);
        getEl("btnRestart").addEventListener("click", restartDaemon, false);
        getEl("btnDaemonStop").addEventListener("click", stopDaemon, false);
        getEl("btnDaemonStart").addEventListener("click", startDaemon, false);
        getEl("btnLiveCapture").addEventListener("click", liveCapture, false);
        getEl("btnLogs").addEventListener("click", openLogs, false);
        getEl("btnLogsRefresh").addEventListener("click", openLogs, false);
        getEl("btnLogsClose").addEventListener("click", closeLogs, false);
        getEl("btnSaveRaw").addEventListener("click", saveRawConfig, false);
        getEl("btnCaptureCancel").addEventListener("click", cancelCapture, false);
        getEl("btnActionCancel").addEventListener("click", function() {
            hideOverlay("actionOverlay"); pendingSlot = null; continuousCapture = false;
        }, false);

        getEl("btnAddMany").addEventListener("click", function() {
            continuousCapture = true;
            captureNewButton(false);
        }, false);

        var addOpts = document.querySelectorAll(".add-opt");
        for (var i = 0; i < addOpts.length; i++) addOpts[i].addEventListener("click", onAddOpt, false);

        getEl("bindingsList").addEventListener("click", onBindingsListClick, false);
        getEl("deviceList").addEventListener("click", onAvailableDeviceClick, false);
        getEl("configuredDevices").addEventListener("click", onConfiguredDeviceClick, false);
        getEl("actionList").addEventListener("click", onActionItemClick, false);

        getEl("bindingsDevicePills").addEventListener("click", function(e) {
            var target = e.target;
            if (!target) return;
            if (target.getAttribute("data-action") === "new") {
                openDeviceDetailNew("", "");
                return;
            }
            var id = target.getAttribute("data-dev-id");
            if (id) {
                currentDeviceId = id;
                renderBindings();
            }
        }, false);

        getEl("btnNewDevice").addEventListener("click", function() {
            openDeviceDetailNew("", "");
        }, false);
        getEl("btnDeviceSave").addEventListener("click", saveDeviceDetail, false);
        getEl("btnDeviceDelete").addEventListener("click", deleteDeviceDetail, false);
        getEl("btnDeviceCancel").addEventListener("click", closeDeviceDetail, false);
        getEl("devDetailGrab").addEventListener("click", toggleDeviceDetailGrab, false);
        getEl("devDetailName").addEventListener("input", function() {
            if (!editingDeviceId) updateDeviceIdView();
        }, false);
        getEl("devDetailName").addEventListener("keyup", function() {
            if (!editingDeviceId) updateDeviceIdView();
        }, false);
        getEl("devDetailGrabInfo").addEventListener("click", function() {
            showInfo("Exclusive: take sole ownership of the input device so other apps (e.g. the Kindle reader) don't also react to its events. Recommended for gamepads and remotes when you only want the mapper to handle them.");
        }, false);
        getEl("devDetailLayoutInfo").addEventListener("click", function() {
            showInfo("Keyboard layout: overrides the system 'us' keymap so a non-US keyboard types correctly, even after the reader re-pins the layout. Choose (system default) to leave it untouched. For an Alt+Shift toggle between two layouts, set keyboard_layout to a comma list (e.g. us,ru) in the config editor on the Debug tab.");
        }, false);
        getEl("btnInfoClose").addEventListener("click", function() {
            hideOverlay("infoOverlay");
        }, false);

        var actionTabs = document.querySelectorAll(".action-tab");
        for (var k = 0; k < actionTabs.length; k++) {
            actionTabs[k].addEventListener("click", onActionTabClick, false);
        }

        getEl("btnActionCustomUse").addEventListener("click", function() {
            var cmd = (getEl("actionCustomCmd").value || "").replace(/^\s+|\s+$/g, "");
            if (!cmd) { showMessage("Enter a command first", true); return; }
            if (!pendingSlot) return;
            setValue(pendingSlot.section, pendingSlot.key, cmd);
            hideOverlay("actionOverlay");
            pendingSlot = null;
            renderBindings();
            showMessage("Mapping added (unsaved)", false);
        }, false);

        var search = getEl("actionSearch");
        search.addEventListener("input", onActionSearchInput, false);
        search.addEventListener("keyup", onActionSearchInput, false);

        getEl("devDetailLayout").addEventListener("click", openLayoutPicker, false);
        getEl("layoutList").addEventListener("click", onLayoutItemClick, false);
        getEl("btnLayoutCancel").addEventListener("click", function() { hideOverlay("layoutOverlay"); }, false);
        var layoutSearch = getEl("layoutSearch");
        var onLayoutSearch = function() { renderLayoutList(getEl("layoutSearch").value); };
        layoutSearch.addEventListener("input", onLayoutSearch, false);
        layoutSearch.addEventListener("keyup", onLayoutSearch, false);
    }

    function sizeTabContent() {
        var vh = window.innerHeight || document.documentElement.clientHeight || 1448;
        var header = document.querySelector(".header");
        var bar    = document.querySelector(".tab-bar");
        var footer = document.querySelector(".footer");
        var used = (header ? header.offsetHeight : 0)
                 + (bar    ? bar.offsetHeight    : 0)
                 + (footer ? footer.offsetHeight : 0)
                 + 20; // small breathing room
        var h = Math.max(200, vh - used);
        var panes = document.querySelectorAll(".tab-content");
        for (var i = 0; i < panes.length; i++) panes[i].style.height = h + "px";

        document.documentElement.style.height = vh + "px";
        document.documentElement.style.overflow = "hidden";
        document.body.style.height = vh + "px";
        document.body.style.overflow = "hidden";
    }

    function bootstrapFetch(attempt) {
        attempt = attempt || 0;
        getJSON("/actions", function(data, err) {
            if (err && attempt < 5) {
                setTimeout(function() { bootstrapFetch(attempt + 1); }, 500);
                return;
            }
            if (data && data.actions) actions = data.actions;
            request("GET", "/config", null, function(text, err2) {
                if (err2) {
                    if (attempt < 5) {
                        setTimeout(function() { bootstrapFetch(attempt + 1); }, 500);
                        return;
                    }
                    showMessage("Cannot load config: " + err2, true);
                    ini = parseIni("");
                } else {
                    ini = parseIni(text);
                }
                migrateLegacy();
                renderBindings();
                renderConfiguredDevices();
            });
        });
    }

    function layoutLabel(code) {
        if (!code) return "(system default)";
        for (var i = 0; i < layouts.length; i++) {
            if (layouts[i].code === code) return (layouts[i].name || code) + " (" + code + ")";
        }
        return code;
    }

    function setDeviceLayout(code) {
        code = code || "";
        var btn = getEl("devDetailLayout");
        btn.setAttribute("data-code", code);
        btn.innerHTML = escapeHtml(layoutLabel(code));
    }

    function renderLayoutList(query) {
        query = (query || "").toLowerCase().replace(/^\s+|\s+$/g, "");
        var html = "";
        if (!query) html += '<div class="action-item" data-code="">(system default)</div>';
        for (var i = 0; i < layouts.length; i++) {
            var code = layouts[i].code;
            var name = layouts[i].name || code;
            if (query && code.toLowerCase().indexOf(query) < 0 && name.toLowerCase().indexOf(query) < 0) continue;
            html += '<div class="action-item" data-code="' + escapeHtml(code) + '">'
                + escapeHtml(name) + ' <span class="action-id">' + escapeHtml(code) + '</span></div>';
        }
        var list = getEl("layoutList");
        list.innerHTML = html;
        list.scrollTop = 0;
    }

    function openLayoutPicker() {
        getEl("layoutSearch").value = "";
        renderLayoutList("");
        showOverlay("layoutOverlay");
    }

    function onLayoutItemClick(e) {
        var target = e.target;
        while (target && target.getAttribute && target.getAttribute("data-code") === null) {
            target = target.parentNode;
        }
        if (!target || !target.getAttribute) return;
        setDeviceLayout(target.getAttribute("data-code"));
        hideOverlay("layoutOverlay");
    }

    function init() {
        bindEvents();
        setTimeout(sizeTabContent, 0);
        bootstrapFetch(0);
        getJSON("/layouts", function(data) {
            if (data && data.layouts) layouts = data.layouts;
        });
        refreshStatus();
    }

    if (document.readyState === "complete" || document.readyState === "interactive") {
        init();
    } else {
        document.addEventListener("DOMContentLoaded", init, false);
    }

    return { refresh: reloadConfig };
})();
