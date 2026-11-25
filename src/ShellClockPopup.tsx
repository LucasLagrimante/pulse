import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";

const WEEKDAY_HEADERS = ["Dom", "Seg", "Ter", "Qua", "Qui", "Sex", "Sáb"];

function buildMonthGrid(year: number, month: number): { day: number; inMonth: boolean; key: string }[] {
  const firstDay = new Date(year, month, 1);
  const startWeekday = firstDay.getDay();
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const prevMonthDays = new Date(year, month, 0).getDate();

  const cells: { day: number; inMonth: boolean; key: string }[] = [];
  for (let i = startWeekday - 1; i >= 0; i -= 1) {
    const day = prevMonthDays - i;
    cells.push({ day, inMonth: false, key: `prev-${day}` });
  }
  for (let day = 1; day <= daysInMonth; day += 1) {
    cells.push({ day, inMonth: true, key: `cur-${day}` });
  }
  while (cells.length < 42) {
    const day = cells.length - (startWeekday + daysInMonth) + 1;
    cells.push({ day, inMonth: false, key: `next-${day}` });
  }
  return cells;
}

export function ShellClockPopup() {
  const [now, setNow] = useState(() => new Date());
  const [viewOffset, setViewOffset] = useState(0);
  const [settingsError, setSettingsError] = useState("");

  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const viewDate = useMemo(
    () => new Date(now.getFullYear(), now.getMonth() + viewOffset, 1),
    [now, viewOffset],
  );
  const viewYear = viewDate.getFullYear();
  const viewMonth = viewDate.getMonth();
  const cells = useMemo(() => buildMonthGrid(viewYear, viewMonth), [viewYear, viewMonth]);

  const timeText = now.toLocaleTimeString("pt-BR", {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
  const fullDateText = now.toLocaleDateString("pt-BR", {
    weekday: "long",
    day: "numeric",
    month: "long",
    year: "numeric",
  });
  const monthLabel = viewDate.toLocaleDateString("pt-BR", {
    month: "long",
    year: "numeric",
  });
  const tzName = Intl.DateTimeFormat().resolvedOptions().timeZone || "Local";

  const isCurrentMonth = viewYear === now.getFullYear() && viewMonth === now.getMonth();
  const todayDay = now.getDate();

  const openSystemDateTime = async () => {
    setSettingsError("");
    try {
      await invoke("open_windows_date_time_settings");
    } catch (error) {
      setSettingsError(
        typeof error === "string" ? error : "Não foi possível abrir as configurações de data e hora.",
      );
    }
  };

  return (
    <section className="shell-clock-popup" aria-label="Calendário e relógio do sistema">
      <div className="shell-clock-hero">
        <strong className="shell-clock-time">{timeText}</strong>
        <p className="shell-clock-date">{fullDateText}</p>
        <span className="shell-clock-tz">Fuso horário: {tzName}</span>
      </div>

      <div className="shell-calendar-card">
        <div className="shell-calendar-head">
          <button
            type="button"
            className="shell-calendar-nav"
            aria-label="Mês anterior"
            onClick={() => setViewOffset(v => v - 1)}
          >
            ‹
          </button>
          <button
            type="button"
            className="shell-calendar-month"
            title="Voltar para o mês atual"
            onClick={() => setViewOffset(0)}
          >
            {monthLabel}
          </button>
          <button
            type="button"
            className="shell-calendar-nav"
            aria-label="Próximo mês"
            onClick={() => setViewOffset(v => v + 1)}
          >
            ›
          </button>
        </div>

        <div className="shell-calendar-weekdays" aria-hidden="true">
          {WEEKDAY_HEADERS.map(day => (
            <span key={day}>{day}</span>
          ))}
        </div>

        <div className="shell-calendar-grid">
          {cells.map(cell => {
            const isToday = isCurrentMonth && cell.inMonth && cell.day === todayDay;
            return (
              <span
                key={cell.key}
                className={`shell-calendar-cell${cell.inMonth ? "" : " is-outside"}${isToday ? " is-today" : ""}`}
                aria-current={isToday ? "date" : undefined}
              >
                {cell.day}
              </span>
            );
          })}
        </div>
      </div>

      <div className="shell-clock-footer">
        <button
          type="button"
          className="shell-clock-settings-btn"
          onClick={() => void openSystemDateTime()}
        >
          Configurações de data e hora do Windows
        </button>
        {settingsError && (
          <p className="flyout-audio-error" role="alert">
            {settingsError}
          </p>
        )}
      </div>
    </section>
  );
}
