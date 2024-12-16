document$.subscribe(function () {
  // Stop mkdocs-material from hiding the version selector when scrolling down;
  // this is done by removing the --active class when mkdocs-material adds it.
  // (--active = scrolled down)

  const title = document.querySelector('*[data-md-component="header-title"]');

  const observer = new MutationObserver((mutations) => {
    mutations.forEach((mutation) => {
      if (mutation.attributeName === "class") {
        const classList = title.className.split(" ");
        classList.forEach((className) => {
          if (className.endsWith("--active")) {
            title.classList.remove(className);
          }
        });
      }
    });
  });

  observer.observe(title, { attributes: true });
});
