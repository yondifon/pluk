import "./confirm.css";
import { invoke } from "../host";
import { renderClosed, renderConfirm, type ConfirmChoice, type ConfirmQuestion } from "./view";

const root = document.getElementById("confirm")!;

/** The question this window was opened for. */
function questionId(): string {
  return new URLSearchParams(window.location.search).get("id") ?? "";
}

async function show(id: string): Promise<void> {
  const [question, answerWindow] = await Promise.all([
    invoke<ConfirmQuestion | null>("confirm_question", { id }),
    invoke<number>("confirm_answer_window"),
  ]);
  if (!question) {
    renderClosed(root, "This request has closed.", "Nothing ran. The agent can ask again.");
    return;
  }

  let answered = false;
  const answer = (choice: ConfirmChoice) => {
    if (answered) return;
    answered = true;
    window.clearInterval(ticker);
    view.settle();
    void invoke("confirm_answer", { id, choice });
  };

  const view = renderConfirm(root, question, answer);
  const closesAt = Date.now() + answerWindow * 1000;
  view.setSecondsLeft(answerWindow);
  const ticker = window.setInterval(() => {
    const left = (closesAt - Date.now()) / 1000;
    view.setSecondsLeft(left);
    if (left <= 0) window.clearInterval(ticker);
  }, 1000);

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
