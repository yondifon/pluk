import "./confirm.css";
import { invoke } from "../host";
import { renderClosed, renderConfirm, type ConfirmChoice, type ConfirmQuestion } from "./view";

const root = document.getElementById("confirm")!;

/** The question this window was opened for. */
function questionId(): string {
  return new URLSearchParams(window.location.search).get("id") ?? "";
}

async function show(id: string): Promise<void> {
  const question = await invoke<ConfirmQuestion | null>("confirm_question", { id });
  if (!question) {
    renderClosed(root, "This request has closed.", "Nothing ran. The agent can ask again.");
    return;
  }

  let answered = false;
  const answer = (choice: ConfirmChoice) => {
    if (answered) return;
    answered = true;
    view.settle();
    void invoke("confirm_answer", { id, choice });
  };

  const view = renderConfirm(root, question, answer);

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") answer("deny");
  });
}

void show(questionId()).catch(() => {
  renderClosed(
    root,
    "Pluk couldn’t load this request.",
    "Nothing ran. Close this window and ask again.",
  );
});
