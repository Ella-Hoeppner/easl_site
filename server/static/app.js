// Small enhancements for the app's server-rendered pages.
//
// - Buttons with `data-api="<url>"` call the JSON API (`data-method`,
//   default POST), after a `data-confirm` prompt if given, then go to
//   `data-redirect` or reload.
// - Forms with `data-api-form` send their fields as JSON to `data-url`
//   (with `{field}` placeholders filled from the form, which are then left
//   out of the body) using `data-method`, then reload.
// - In a playlist the owner can edit, `data-move="up|down"` buttons reorder
//   items and save the new order.
// - Clicking outside an open account menu closes it.

const toast = document.getElementById("toast");
let toastTimer = 0;

/** Shows a short message at the bottom of the page. */
function showToast(message, isError) {
  if (!toast) return;
  toast.textContent = message;
  toast.classList.toggle("error-toast", Boolean(isError));
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => {
    toast.hidden = true;
  }, 4000);
}

/** Calls the JSON API, returning the parsed response or throwing its error. */
async function callApi(method, url, body) {
  const options = {
    method,
    credentials: "same-origin",
    headers: { Accept: "application/json" },
  };
  if (body !== undefined) {
    options.headers["Content-Type"] = "application/json";
    options.body = JSON.stringify(body);
  }
  const response = await fetch(url, options);
  if (response.status === 204) return null;
  let data = null;
  try {
    data = await response.json();
  } catch {
    data = null;
  }
  if (!response.ok) {
    const message =
      (data && data.error) || `The request failed (${response.status}).`;
    throw new Error(message);
  }
  return data;
}

async function runApiButton(button) {
  const confirmText = button.dataset.confirm;
  if (confirmText && !window.confirm(confirmText)) return;
  button.disabled = true;
  try {
    await callApi(button.dataset.method || "POST", button.dataset.api);
    if (button.dataset.redirect) {
      window.location.assign(button.dataset.redirect);
    } else {
      window.location.reload();
    }
  } catch (error) {
    showToast(error.message, true);
    button.disabled = false;
  }
}

async function submitApiForm(form) {
  const fields = Object.fromEntries(new FormData(form).entries());
  const used = new Set();
  const url = form.dataset.url.replace(/\{(\w+)\}/g, (_, name) => {
    used.add(name);
    return encodeURIComponent(String(fields[name] ?? "").trim());
  });
  const body = {};
  for (const [name, value] of Object.entries(fields)) {
    if (!used.has(name)) body[name] = value;
  }
  const submit = form.querySelector('[type="submit"]');
  if (submit) submit.disabled = true;
  try {
    const method = form.dataset.method || "POST";
    const hasBody = Object.keys(body).length > 0;
    await callApi(method, url, hasBody ? body : undefined);
    window.location.reload();
  } catch (error) {
    showToast(error.message, true);
    if (submit) submit.disabled = false;
  }
}

async function moveItem(button) {
  const item = button.closest(".playlist-item");
  const list = button.closest(".playlist-items");
  if (!item || !list) return;
  const sibling =
    button.dataset.move === "up"
      ? item.previousElementSibling
      : item.nextElementSibling;
  if (!sibling) return;
  if (button.dataset.move === "up") {
    list.insertBefore(item, sibling);
  } else {
    list.insertBefore(sibling, item);
  }
  button.focus();
  const order = [...list.querySelectorAll(".playlist-item")].map(
    (element) => element.dataset.sketch,
  );
  try {
    await callApi("PUT", `/api/playlists/${list.dataset.playlist}/order`, {
      sketch_ids: order,
    });
  } catch (error) {
    showToast(error.message, true);
    window.location.reload();
  }
}

document.addEventListener("click", (event) => {
  const target = event.target instanceof Element ? event.target : null;
  if (!target) return;
  const apiButton = target.closest("[data-api]");
  if (apiButton) {
    event.preventDefault();
    runApiButton(apiButton);
    return;
  }
  const moveButton = target.closest("[data-move]");
  if (moveButton) {
    event.preventDefault();
    moveItem(moveButton);
    return;
  }
  for (const menu of document.querySelectorAll("details.user-menu[open]")) {
    if (!menu.contains(target)) menu.open = false;
  }
});

document.addEventListener("submit", (event) => {
  const form = event.target;
  if (form instanceof HTMLFormElement && form.hasAttribute("data-api-form")) {
    event.preventDefault();
    submitApiForm(form);
  }
});

document.addEventListener("keydown", (event) => {
  if (event.key !== "Escape") return;
  for (const menu of document.querySelectorAll("details.user-menu[open]")) {
    menu.open = false;
  }
});
