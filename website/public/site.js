async function copyText(text) {
  if (navigator.clipboard?.writeText) {
    await navigator.clipboard.writeText(text);
    return;
  }

  const input = document.createElement("textarea");
  input.value = text;
  input.setAttribute("readonly", "");
  input.style.position = "fixed";
  input.style.opacity = "0";
  document.body.append(input);
  input.select();
  const copied = document.execCommand("copy");
  input.remove();
  if (!copied) throw new Error("clipboard copy was refused");
}

function enableCopyCommands() {
  for (const button of document.querySelectorAll("[data-copy-command]")) {
    button.dataset.copyReady = "true";
  }
}

window.addEventListener("load", () => window.setTimeout(enableCopyCommands, 750), { once: true });

document.addEventListener("click", async (event) => {
  const button = event.target.closest?.("[data-copy-command][data-copy-ready=true]");
  if (!button) return;

  const label = button.querySelector("b");
  try {
    await copyText(button.dataset.copyCommand);
    label.textContent = "Copied";
  } catch {
    label.textContent = "Select";
  }
  window.setTimeout(() => { label.textContent = "Copy"; }, 1800);
});
