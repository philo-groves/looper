type BlankWorkspacePageProps = {
  title: string;
};

export function BlankWorkspacePage({ title }: BlankWorkspacePageProps) {
  return (
    <section className="rounded-xl border border-zinc-300 bg-white p-5 dark:border-zinc-700 dark:bg-zinc-950">
      <h1 className="text-xl font-semibold">{title}</h1>
      <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">Coming soon.</p>
    </section>
  );
}
