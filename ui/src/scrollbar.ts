/**
 * A thin scrollbar that is only drawn while the container is scrolling,
 * the way overlay scrollbars behave, for panes the app scrolls itself.
 */
const SETTLE_MS = 700;

export function fadeScrollbar(container: HTMLElement): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const onScroll = (): void => {
    container.classList.add("is-scrolling");
    if (timer !== null) {
      clearTimeout(timer);
    }
    timer = setTimeout(() => {
      container.classList.remove("is-scrolling");
      timer = null;
    }, SETTLE_MS);
  };
  container.addEventListener("scroll", onScroll, { passive: true });
  return () => {
    container.removeEventListener("scroll", onScroll);
    if (timer !== null) {
      clearTimeout(timer);
    }
  };
}
