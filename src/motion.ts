// Motor de movimento: adiciona .in aos elementos .reveal quando entram na
// viewport (IntersectionObserver — sem listener de scroll). Usado pelas
// janelas que importam tokens.css e aplicam as classes .reveal/.stag.
export function observeReveals(root: ParentNode = document): void {
  const els = Array.from(root.querySelectorAll<HTMLElement>(".reveal"));
  if (!("IntersectionObserver" in window)) {
    els.forEach((el) => el.classList.add("in"));
    return;
  }
  const io = new IntersectionObserver(
    (entries) => {
      entries.forEach((entry) => {
        if (entry.isIntersecting) {
          (entry.target as HTMLElement).classList.add("in");
          io.unobserve(entry.target);
        }
      });
    },
    { threshold: 0.12 },
  );
  els.forEach((el) => io.observe(el));
}
