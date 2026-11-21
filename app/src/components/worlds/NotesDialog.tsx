import { Copy, Plus, X } from "lucide-react";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Button } from "@/components/ui/Button";
import { Checkbox } from "@/components/ui/Checkbox";
import { Dialog } from "@/components/ui/Dialog";
import { TextArea, TextInput } from "@/components/ui/Field";
import { IconButton } from "@/components/ui/IconButton";
import { Select } from "@/components/ui/Select";
import { unwrap } from "@/lib/api";
import { commands, type Dimension, type Todo } from "@/lib/bindings";
import { reportError } from "@/lib/report";
import { useGameStore } from "@/stores/game";

export interface NotesTarget {
  folder: string;
  name: string;
  /** 世界种子（文字），读不到为空 */
  seed?: string | null;
}

/** 正在编辑的坐标：数字先按文字存，打「-」的时候不会被吃掉 */
interface PlaceDraft {
  key: number;
  name: string;
  x: string;
  y: string;
  z: string;
  dimension: Dimension;
}

interface TodoDraft extends Todo {
  key: number;
}

let nextKey = 1;
const keyed = () => nextKey++;

const toNumber = (text: string) => {
  const value = Number.parseInt(text, 10);
  return Number.isFinite(value) ? value : 0;
};

/** 小标题：面板里的分组小字（12 号灰色） */
function Label({ children }: { children: string }) {
  return <h3 className="mb-2 font-medium text-fg-muted text-xs">{children}</h3>;
}

/**
 * 世界笔记：种子（从存档里读的，能复制）、待办、坐标、随手记。
 * 首页世界名下面显示第一条没做完的待办（没有就是随手记的第一行）。
 */
export function NotesDialog({
  instanceId,
  world,
  onClose,
}: {
  instanceId: string;
  /** 写哪个世界的笔记；null 时弹窗关着 */
  world: NotesTarget | null;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const refreshRecentWorlds = useGameStore((s) => s.refreshRecentWorlds);
  const [todos, setTodos] = useState<TodoDraft[]>([]);
  const [places, setPlaces] = useState<PlaceDraft[]>([]);
  const [text, setText] = useState("");
  const [newTodo, setNewTodo] = useState("");
  const [loaded, setLoaded] = useState(false);
  const [saving, setSaving] = useState(false);

  const folder = world?.folder;
  useEffect(() => {
    if (!folder) return;
    let cancelled = false;
    setLoaded(false);
    setNewTodo("");
    commands
      .worldsNotesGet(instanceId, folder)
      .then(unwrap)
      .then((notes) => {
        if (cancelled) return;
        setTodos((notes.todos ?? []).map((todo) => ({ ...todo, key: keyed() })));
        setPlaces(
          (notes.places ?? []).map((p) => ({
            key: keyed(),
            name: p.name,
            x: String(p.x),
            y: String(p.y),
            z: String(p.z),
            dimension: p.dimension ?? "overworld",
          })),
        );
        setText(notes.text ?? "");
        setLoaded(true);
      })
      .catch((error: unknown) => reportError(error, "读取世界笔记"));
    return () => {
      cancelled = true;
    };
  }, [instanceId, folder]);

  const copy = (value: string) => {
    navigator.clipboard
      .writeText(value)
      .then(() => toast.success(t("notes.copied")))
      .catch((error: unknown) => reportError(error, "复制"));
  };

  const addTodo = () => {
    const value = newTodo.trim();
    if (!value) return;
    setTodos((list) => [...list, { key: keyed(), text: value, done: false }]);
    setNewTodo("");
  };

  const updatePlace = (key: number, change: Partial<PlaceDraft>) =>
    setPlaces((list) => list.map((p) => (p.key === key ? { ...p, ...change } : p)));

  const save = async () => {
    if (!world) return;
    setSaving(true);
    // 输入框里还有没按回车的待办：一起存
    const pending = newTodo.trim();
    const allTodos = pending ? [...todos, { key: 0, text: pending, done: false }] : todos;
    try {
      unwrap(
        await commands.worldsNotesSet(instanceId, world.folder, {
          todos: allTodos.map(({ text: todo, done }) => ({ text: todo, done })),
          places: places.map((p) => ({
            name: p.name,
            x: toNumber(p.x),
            y: toNumber(p.y),
            z: toNumber(p.z),
            dimension: p.dimension,
          })),
          text,
        }),
      );
      toast.success(t("notes.saved"));
      void refreshRecentWorlds();
      onClose();
    } catch (error) {
      reportError(error, "保存世界笔记");
    } finally {
      setSaving(false);
    }
  };

  const dimensions: { value: Dimension; label: string }[] = [
    { value: "overworld", label: t("notes.dimension.overworld") },
    { value: "nether", label: t("notes.dimension.nether") },
    { value: "end", label: t("notes.dimension.end") },
  ];

  return (
    <Dialog
      open={world !== null}
      onClose={onClose}
      wide
      // 一行坐标要放名字、XYZ、维度和两个按钮：比一般的宽弹窗再宽一点
      className="max-w-2xl!"
      title={t("notes.title", { name: world?.name ?? "" })}
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button variant="primary" disabled={!loaded || saving} onClick={() => void save()}>
            {t("notes.save")}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-5">
        {world?.seed && (
          <section>
            <Label>{t("notes.seed")}</Label>
            <div className="flex items-center gap-2">
              <span className="select-text font-mono text-sm">{world.seed}</span>
              <IconButton
                label={t("notes.copy")}
                size="sm"
                onClick={() => world.seed && copy(world.seed)}
              >
                <Copy className="size-4" aria-hidden />
              </IconButton>
            </div>
          </section>
        )}

        <section>
          <Label>{t("notes.todos")}</Label>
          <ul className="flex flex-col gap-2">
            {todos.map((todo) => (
              <li key={todo.key} className="flex items-center gap-3">
                <Checkbox
                  label={todo.text}
                  checked={todo.done ?? false}
                  onChange={(done) =>
                    setTodos((list) =>
                      list.map((t2) => (t2.key === todo.key ? { ...t2, done } : t2)),
                    )
                  }
                />
                <TextInput
                  value={todo.text}
                  aria-label={t("notes.todos")}
                  className={todo.done ? "text-fg-muted line-through" : ""}
                  onChange={(event) =>
                    setTodos((list) =>
                      list.map((t2) =>
                        t2.key === todo.key ? { ...t2, text: event.target.value } : t2,
                      ),
                    )
                  }
                />
                <IconButton
                  label={t("notes.remove")}
                  size="sm"
                  danger
                  onClick={() => setTodos((list) => list.filter((t2) => t2.key !== todo.key))}
                >
                  <X className="size-4" aria-hidden />
                </IconButton>
              </li>
            ))}
            <li className="flex items-center gap-3">
              <span className="size-5 shrink-0" aria-hidden />
              <TextInput
                value={newTodo}
                placeholder={t("notes.todoPlaceholder")}
                aria-label={t("notes.todos")}
                onChange={(event) => setNewTodo(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter" && !event.nativeEvent.isComposing) {
                    event.preventDefault();
                    addTodo();
                  }
                }}
              />
              <IconButton label={t("notes.todos")} size="sm" onClick={addTodo}>
                <Plus className="size-4" aria-hidden />
              </IconButton>
            </li>
          </ul>
        </section>

        <section>
          <div className="mb-2 flex items-center justify-between gap-3">
            <h3 className="font-medium text-fg-muted text-xs">{t("notes.places")}</h3>
            <Button
              variant="secondary"
              size="sm"
              onClick={() =>
                setPlaces((list) => [
                  ...list,
                  { key: keyed(), name: "", x: "0", y: "64", z: "0", dimension: "overworld" },
                ])
              }
            >
              <Plus className="size-3.5" aria-hidden />
              {t("notes.placeAdd")}
            </Button>
          </div>
          <ul className="flex flex-col gap-2">
            {places.map((place) => (
              <li
                key={place.key}
                className="grid grid-cols-[7rem_repeat(3,minmax(0,1fr))_7rem_auto_auto] items-center gap-2"
              >
                <TextInput
                  value={place.name}
                  placeholder={t("notes.placeNamePlaceholder")}
                  aria-label={t("notes.placeName")}
                  onChange={(event) => updatePlace(place.key, { name: event.target.value })}
                />
                {(["x", "y", "z"] as const).map((axis) => (
                  <TextInput
                    key={axis}
                    className="px-2! text-center"
                    inputMode="numeric"
                    value={place[axis]}
                    aria-label={axis.toUpperCase()}
                    placeholder={axis.toUpperCase()}
                    onChange={(event) =>
                      updatePlace(place.key, {
                        [axis]: event.target.value.replace(/[^\d-]/g, ""),
                      })
                    }
                  />
                ))}
                <Select
                  label={t("notes.places")}
                  value={place.dimension}
                  options={dimensions}
                  onChange={(value) => updatePlace(place.key, { dimension: value as Dimension })}
                />
                <IconButton
                  label={t("notes.copyCoords")}
                  size="sm"
                  onClick={() =>
                    copy(`${toNumber(place.x)} ${toNumber(place.y)} ${toNumber(place.z)}`)
                  }
                >
                  <Copy className="size-4" aria-hidden />
                </IconButton>
                <IconButton
                  label={t("notes.remove")}
                  size="sm"
                  danger
                  onClick={() => setPlaces((list) => list.filter((p) => p.key !== place.key))}
                >
                  <X className="size-4" aria-hidden />
                </IconButton>
              </li>
            ))}
          </ul>
        </section>

        <section>
          <Label>{t("notes.text")}</Label>
          <TextArea
            rows={4}
            value={text}
            aria-label={t("notes.text")}
            onChange={(event) => setText(event.target.value)}
          />
        </section>
      </div>
    </Dialog>
  );
}
