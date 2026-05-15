import React, { useEffect } from 'react';
import { Check, AlertTriangle, AlertCircle, Info, X } from 'lucide-react';
import './Toaster.css';

/**
 * Lightweight toast queue. Max 3 visible; older toasts slide out as new ones arrive.
 * Auto-dismiss timer is owned per-toast in the queue (App.jsx schedules removal).
 *
 * Type variants: success (green) | warning (gold) | error (red) | info (neutral)
 * Backward-compat: legacy callers passing { msg, type } still work — Toaster
 * accepts both a single toast object (legacy) and an array.
 */

const TYPE_META = {
  success: { Icon: Check,          className: 'toast-success' },
  info:    { Icon: Info,           className: 'toast-info' },
  warning: { Icon: AlertTriangle,  className: 'toast-warning' },
  error:   { Icon: AlertCircle,    className: 'toast-error' },
};

function Toast({ toast, onDismiss }) {
  const meta = TYPE_META[toast.type] || TYPE_META.success;
  const Icon = meta.Icon;

  // Pause auto-dismiss on hover (still owned by App.jsx; this just gives the
  // toast a way to extend its life via mouseenter, which we don't implement
  // here to keep the queue logic simple — fixed 3.5s default).

  return (
    <div className={`toast ${meta.className}`} role="status" aria-live="polite">
      <span className="toast-icon" aria-hidden="true">
        <Icon size={14} strokeWidth={2} />
      </span>
      <span className="toast-msg">{toast.msg}</span>
      <button
        className="toast-dismiss"
        type="button"
        onClick={() => onDismiss(toast.id)}
        aria-label="Dismiss"
      >
        <X size={12} strokeWidth={2} />
      </button>
    </div>
  );
}

export default function Toaster({ toasts, onDismiss }) {
  // Normalise: accept array, single object, or null
  const list = Array.isArray(toasts)
    ? toasts
    : (toasts ? [{ ...toasts, id: toasts.id ?? 0 }] : []);

  if (list.length === 0) return null;

  return (
    <div className="toaster" aria-live="polite">
      {list.map(t => (
        <Toast key={t.id} toast={t} onDismiss={onDismiss} />
      ))}
    </div>
  );
}
