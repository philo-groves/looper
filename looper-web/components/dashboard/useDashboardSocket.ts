"use client";

import { useCallback, useEffect, useRef, useState } from "react";

import {
  DashboardPayload,
  DashboardResponse,
  LoopPhaseTransitionPayload,
} from "@/components/dashboard/types";

type WsResponseEnvelope = {
  type: "response";
  id?: number;
  ok?: boolean;
  result?: unknown;
  error?: string;
};

function resolveWsUrl() {
  const configured = process.env.NEXT_PUBLIC_LOOPER_AGENT_WS_URL;
  if (configured && configured.length > 0) {
    return configured;
  }
  return "ws://127.0.0.1:10001/api/ws";
}

export function useDashboardSocket(
  onSnapshot?: (snapshot: DashboardPayload) => void,
) {
  const [data, setData] = useState<DashboardPayload | null>(null);
  const [socketConnected, setSocketConnected] = useState(false);
  const [socketError, setSocketError] = useState<string | null>(null);
  const phaseQueueRef = useRef<LoopPhaseTransitionPayload[]>([]);
  const phaseTimerRef = useRef<number | null>(null);
  const latestPhaseStartRef = useRef<number>(0);

  const flushPhaseQueue = useCallback(() => {
    if (phaseTimerRef.current !== null) {
      return;
    }

    const processNext = () => {
      const next = phaseQueueRef.current.shift();
      if (!next) {
        phaseTimerRef.current = null;
        return;
      }

      setData((current) => {
        if (!current) {
          latestPhaseStartRef.current = next.loop_visualization.current_phase_started_at_unix_ms;
          return current;
        }
        latestPhaseStartRef.current = next.loop_visualization.current_phase_started_at_unix_ms;
        return {
          ...current,
          loop_visualization: next.loop_visualization,
        };
      });

      phaseTimerRef.current = window.setTimeout(processNext, 200);
    };

    phaseTimerRef.current = window.setTimeout(processNext, 0);
  }, []);

  const wsCommand = useCallback(async function wsCommand<T>(method: string, params: unknown): Promise<T> {
    const socket = new window.WebSocket(resolveWsUrl());

    return new Promise<T>((resolve, reject) => {
      let done = false;

      function finishError(message: string) {
        if (done) {
          return;
        }
        done = true;
        socket.close();
        reject(new Error(message));
      }

      function finishOk(value: T) {
        if (done) {
          return;
        }
        done = true;
        socket.close();
        resolve(value);
      }

      socket.onerror = () => finishError("websocket request failed");

      socket.onopen = () => {
        socket.send(
          JSON.stringify({
            id: 1,
            method,
            params,
          }),
        );
      };

      socket.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data) as WsResponseEnvelope | DashboardResponse;
          if (payload.type !== "response" || payload.id !== 1) {
            return;
          }

          if (!payload.ok) {
            finishError(payload.error ?? "request failed");
            return;
          }

          finishOk(payload.result as T);
        } catch {
          finishError("invalid websocket response payload");
        }
      };
    });
  }, []);

  useEffect(() => {
    let ws: WebSocket | null = null;
    let reconnectTimer: number | null = null;
    let closedByCleanup = false;

    function connect() {
      ws = new window.WebSocket(resolveWsUrl());

      ws.onopen = () => {
        setSocketConnected(true);
        setSocketError(null);
      };

      ws.onmessage = (event) => {
        try {
          const payload = JSON.parse(event.data) as DashboardResponse;
          if (payload.type === "event" && payload.event === "dashboard_snapshot" && payload.data) {
            const snapshot = payload.data;
            setData((current) => {
              if (!current) {
                latestPhaseStartRef.current =
                  snapshot.loop_visualization.current_phase_started_at_unix_ms;
                return snapshot;
              }

              if (
                snapshot.loop_visualization.current_phase_started_at_unix_ms <=
                latestPhaseStartRef.current
              ) {
                return {
                  ...snapshot,
                  loop_visualization: current.loop_visualization,
                };
              }

              latestPhaseStartRef.current =
                snapshot.loop_visualization.current_phase_started_at_unix_ms;
              return snapshot;
            });
            onSnapshot?.(snapshot);
            return;
          }

          if (payload.type === "event" && payload.event === "loop_phase_transition" && payload.data) {
            const phaseEvent = payload.data as unknown as LoopPhaseTransitionPayload;

            const isNoSurpriseIdleTransition =
              phaseEvent.phase === "idle" &&
              phaseEvent.loop_visualization.local_current_step === "no_surprise" &&
              !phaseEvent.loop_visualization.action_required;

            if (isNoSurpriseIdleTransition) {
              phaseQueueRef.current.push(phaseEvent);
              phaseQueueRef.current.push({
                ...phaseEvent,
                loop_visualization: {
                  ...phaseEvent.loop_visualization,
                  local_current_step: "gather_new_percepts",
                },
              });
            } else {
              phaseQueueRef.current.push(phaseEvent);
            }

            flushPhaseQueue();
          }
        } catch {
          setSocketError("Received invalid websocket payload.");
        }
      };

      ws.onerror = () => {
        setSocketConnected(false);
        setSocketError("Websocket connection error.");
      };

      ws.onclose = () => {
        setSocketConnected(false);
        if (!closedByCleanup) {
          reconnectTimer = window.setTimeout(connect, 1200);
        }
      };
    }

    connect();

    return () => {
      closedByCleanup = true;
      if (reconnectTimer !== null) {
        window.clearTimeout(reconnectTimer);
      }
      if (phaseTimerRef.current !== null) {
        window.clearTimeout(phaseTimerRef.current);
      }
      phaseTimerRef.current = null;
      phaseQueueRef.current = [];
      ws?.close();
    };
  }, [flushPhaseQueue, onSnapshot]);

  return {
    data,
    socketConnected,
    socketError,
    wsCommand,
  };
}
