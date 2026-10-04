// The copy buttons beside the install commands. What they say, in the
// page's language, is on the button itself.
for (const button of document.querySelectorAll(".copy")) {
  const label = button.textContent;
  button.addEventListener("click", async () => {
    const text = button.parentElement.querySelector("code").textContent;
    try {
      await navigator.clipboard.writeText(text);
      button.textContent = button.dataset.copied;
    } catch {
      const range = document.createRange();
      range.selectNodeContents(button.parentElement.querySelector("code"));
      const selection = getSelection();
      selection.removeAllRanges();
      selection.addRange(range);
      button.textContent = button.dataset.selected;
    }
    setTimeout(() => (button.textContent = label), 1600);
  });
}
