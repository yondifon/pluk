import "./confirm.css";
import { invoke, listen } from "../host";
import {
  renderClosed,
  renderConfirm,
  renderPost,
  type ConfirmChoice,
  type ConfirmQuestion,
  type PostChoice,
  type PostQuestion,
} from "./view";

const root = document.getElementById("confirm")!;

/** The question this window was opened for: a refused call, or a post. */
function opened(): { kind: "call"; id: string } | { kind: "post"; id: string } {
  const params = new URLSearchParams(window.location.search);
  const post = params.get("post");
  if (post) return { kind: "post", id: post };
  return { kind: "call", id: params.get("id") ?? "" };
}

/** Ask about one post. Escape and closing the window decide nothing. */
async function showPost(id: string): Promise<void> {
  const question = await invoke<PostQuestion | null>("wande_question", { id });
  if (!question) {
    renderClosed(root, "This post has closed.", "Nothing was posted. Check the Wande panel in Pluk.");
    return;
  }

  let answered = false;
  const answer = (choice: PostChoice) => {
    if (answered) return;
    answered = true;
    window.clearInterval(ticker);
    view.settle();
    void invoke("wande_answer", { id, choice });
  };

  const view = renderPost(root, question, answer);
  view.setSecondsLeft((question.closesAt - Date.now()) / 1000);
  const ticker = window.setInterval(() => {
    const left = (question.closesAt - Date.now()) / 1000;
    view.setSecondsLeft(left);
    if (left <= 0) window.clearInterval(ticker);
  }, 1000);

  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") answer("later");
  });
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

const target = opened();
const run = target.kind === "post" ? showPost(target.id) : show(target.id);
void run.catch(() => {
  renderClosed(
    root,
    "Pluk couldn’t load this request.",
    "Nothing ran. Close this window and ask again.",
  );
});
// An open window is handed the next post rather than reopened.
void listen<string>("pluk://wande-question", (id) => void showPost(id));
