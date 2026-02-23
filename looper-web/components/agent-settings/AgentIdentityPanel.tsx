"use client";

import { ChangeEvent, useEffect, useMemo, useState } from "react";

import { useDashboardSocket } from "@/components/dashboard/useDashboardSocket";

type SkillSummary = {
  id: string;
  name: string;
  updated_at_unix_ms: number;
};

type AgentIdentityResponse = {
  soul_markdown: string;
  skills: SkillSummary[];
};

type SkillDocumentResponse = {
  id: string;
  name: string;
  markdown: string;
};

function timestampLabel(unixMs: number) {
  if (!unixMs) {
    return "Unknown";
  }
  return new Date(unixMs).toLocaleString();
}

export function AgentIdentityPanel() {
  const { socketConnected, socketError, wsCommand } = useDashboardSocket();

  const [soulMarkdown, setSoulMarkdown] = useState("");
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [selectedSkillId, setSelectedSkillId] = useState<string | null>(null);
  const [skillName, setSkillName] = useState("");
  const [skillMarkdown, setSkillMarkdown] = useState("");
  const [skillUrl, setSkillUrl] = useState("");
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    async function loadIdentity() {
      try {
        const payload = await wsCommand<AgentIdentityResponse>("get_agent_identity", {});
        if (cancelled) {
          return;
        }
        setSoulMarkdown(payload.soul_markdown);
        setSkills(payload.skills);
        if (!selectedSkillId && payload.skills.length > 0) {
          setSelectedSkillId(payload.skills[0].id);
        }
      } catch (loadError) {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : "Failed to load agent identity.");
        }
      }
    }

    void loadIdentity();
    return () => {
      cancelled = true;
    };
  }, [selectedSkillId, wsCommand]);

  useEffect(() => {
    if (!selectedSkillId) {
      setSkillName("");
      setSkillMarkdown("");
      return;
    }

    let cancelled = false;
    async function loadSkill() {
      try {
        const payload = await wsCommand<SkillDocumentResponse>("get_skill", {
          id: selectedSkillId,
        });
        if (cancelled) {
          return;
        }
        setSkillName(payload.name);
        setSkillMarkdown(payload.markdown);
      } catch (loadError) {
        if (!cancelled) {
          setError(loadError instanceof Error ? loadError.message : "Failed to load skill.");
        }
      }
    }

    void loadSkill();
    return () => {
      cancelled = true;
    };
  }, [selectedSkillId, wsCommand]);

  async function refreshIdentity() {
    const payload = await wsCommand<AgentIdentityResponse>("get_agent_identity", {});
    setSoulMarkdown(payload.soul_markdown);
    setSkills(payload.skills);
  }

  async function saveSoul() {
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      await wsCommand("save_soul_markdown", {
        markdown: soulMarkdown,
      });
      setStatus("Soul markdown saved.");
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : "Failed to save soul markdown.");
    } finally {
      setBusy(false);
    }
  }

  async function saveSkill() {
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      const payload = await wsCommand<SkillDocumentResponse>("save_skill", {
        id: selectedSkillId,
        name: skillName,
        markdown: skillMarkdown,
      });
      await refreshIdentity();
      setSelectedSkillId(payload.id);
      setSkillName(payload.name);
      setStatus("Skill saved.");
    } catch (saveError) {
      setError(saveError instanceof Error ? saveError.message : "Failed to save skill.");
    } finally {
      setBusy(false);
    }
  }

  async function deleteSkill() {
    if (!selectedSkillId) {
      return;
    }
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      await wsCommand("delete_skill", { id: selectedSkillId });
      await refreshIdentity();
      setSelectedSkillId(null);
      setSkillName("");
      setSkillMarkdown("");
      setStatus("Skill deleted.");
    } catch (deleteError) {
      setError(deleteError instanceof Error ? deleteError.message : "Failed to delete skill.");
    } finally {
      setBusy(false);
    }
  }

  async function importFromUrl() {
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      const payload = await wsCommand<SkillDocumentResponse>("save_skill_from_url", {
        url: skillUrl,
        name: skillName,
      });
      await refreshIdentity();
      setSelectedSkillId(payload.id);
      setSkillName(payload.name);
      setSkillMarkdown(payload.markdown);
      setSkillUrl("");
      setStatus("Skill imported from URL.");
    } catch (importError) {
      setError(importError instanceof Error ? importError.message : "Failed to import skill.");
    } finally {
      setBusy(false);
    }
  }

  async function handleSkillFileUpload(event: ChangeEvent<HTMLInputElement>) {
    const file = event.target.files?.[0];
    if (!file) {
      return;
    }

    const markdown = await file.text();
    setBusy(true);
    setError(null);
    setStatus(null);
    try {
      const payload = await wsCommand<SkillDocumentResponse>("save_skill", {
        id: null,
        name: file.name,
        markdown,
      });
      await refreshIdentity();
      setSelectedSkillId(payload.id);
      setSkillName(payload.name);
      setSkillMarkdown(payload.markdown);
      setStatus("Skill uploaded.");
    } catch (uploadError) {
      setError(uploadError instanceof Error ? uploadError.message : "Failed to upload skill.");
    } finally {
      setBusy(false);
      event.target.value = "";
    }
  }

  const canSaveSkill = useMemo(
    () => skillMarkdown.trim().length > 0 && !busy,
    [busy, skillMarkdown],
  );

  return (
    <section className="space-y-4">
      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h1 className="text-xl font-semibold">Agent Identity</h1>
        <p className="mt-2 text-sm text-zinc-600 dark:text-zinc-300">
          Edit the Soul markdown and manage skill markdown files stored in <code>.agents/skills</code>.
        </p>
      </article>

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h2 className="text-base font-semibold">Soul</h2>
        <textarea
          value={soulMarkdown}
          onChange={(event) => setSoulMarkdown(event.target.value)}
          rows={16}
          className="mt-3 w-full rounded-md border border-zinc-300 bg-white px-3 py-2 font-mono text-sm dark:border-zinc-700 dark:bg-zinc-900"
        />
        <div className="mt-3 flex items-center gap-2">
          <button
            type="button"
            onClick={() => void saveSoul()}
            disabled={busy || !socketConnected}
            className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
          >
            Save Soul
          </button>
        </div>
      </article>

      <article className="rounded-2xl border border-zinc-300 bg-white p-5 shadow-sm dark:border-zinc-700 dark:bg-zinc-950">
        <h2 className="text-base font-semibold">Skills</h2>
        <div className="mt-3 grid gap-4 lg:grid-cols-12">
          <div className="space-y-2 lg:col-span-4">
            <div className="flex gap-2">
              <button
                type="button"
                onClick={() => {
                  setSelectedSkillId(null);
                  setSkillName("new-skill");
                  setSkillMarkdown("# New Skill\n\nDescribe what this skill does.");
                }}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800"
              >
                New Skill
              </button>
              <label className="cursor-pointer rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800">
                Upload File
                <input type="file" accept=".md,text/markdown" className="hidden" onChange={handleSkillFileUpload} />
              </label>
            </div>

            <ul className="max-h-[320px] space-y-2 overflow-y-auto rounded-md border border-zinc-300 p-2 dark:border-zinc-700">
              {skills.length === 0 ? (
                <li className="text-sm text-zinc-600 dark:text-zinc-300">No saved skills yet.</li>
              ) : (
                skills.map((skill) => (
                  <li key={skill.id}>
                    <button
                      type="button"
                      onClick={() => setSelectedSkillId(skill.id)}
                      className={`w-full rounded-md px-2 py-2 text-left text-sm ${
                        selectedSkillId === skill.id
                          ? "bg-zinc-200 dark:bg-zinc-800"
                          : "bg-zinc-100 dark:bg-zinc-900"
                      }`}
                    >
                      <p className="font-medium">{skill.name}</p>
                      <p className="text-xs text-zinc-500 dark:text-zinc-400">
                        Updated {timestampLabel(skill.updated_at_unix_ms)}
                      </p>
                    </button>
                  </li>
                ))
              )}
            </ul>
          </div>

          <div className="space-y-3 lg:col-span-8">
            <input
              type="text"
              value={skillName}
              onChange={(event) => setSkillName(event.target.value)}
              placeholder="Skill name"
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
            />

            <div className="flex gap-2">
              <input
                type="url"
                value={skillUrl}
                onChange={(event) => setSkillUrl(event.target.value)}
                placeholder="https://..."
                className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-900"
              />
              <button
                type="button"
                onClick={() => void importFromUrl()}
                disabled={busy || !socketConnected || skillUrl.trim().length === 0}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
              >
                Import URL
              </button>
            </div>

            <textarea
              value={skillMarkdown}
              onChange={(event) => setSkillMarkdown(event.target.value)}
              rows={16}
              className="w-full rounded-md border border-zinc-300 bg-white px-3 py-2 font-mono text-sm dark:border-zinc-700 dark:bg-zinc-900"
            />

            <div className="flex flex-wrap items-center gap-2">
              <button
                type="button"
                onClick={() => void saveSkill()}
                disabled={!canSaveSkill || !socketConnected}
                className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm font-medium disabled:opacity-50 dark:border-zinc-700 dark:bg-zinc-800"
              >
                Save Skill
              </button>
              <button
                type="button"
                onClick={() => void deleteSkill()}
                disabled={busy || !socketConnected || !selectedSkillId}
                className="rounded-md border border-red-800 bg-red-700 px-3 py-2 text-sm font-medium text-white disabled:opacity-50"
              >
                Delete Skill
              </button>
            </div>
          </div>
        </div>
      </article>

      {!socketConnected && socketError ? (
        <p className="rounded-md border border-red-700 bg-red-700 px-3 py-2 text-sm text-white">{socketError}</p>
      ) : null}
      {error ? (
        <p className="rounded-md border border-red-700 bg-red-700 px-3 py-2 text-sm text-white">{error}</p>
      ) : null}
      {status ? (
        <p className="rounded-md border border-zinc-300 bg-zinc-100 px-3 py-2 text-sm dark:border-zinc-700 dark:bg-zinc-800">
          {status}
        </p>
      ) : null}
    </section>
  );
}
