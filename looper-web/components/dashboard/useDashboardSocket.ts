"use client";

import { useCallback, useEffect, useState } from "react";

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
            setData(snapshot);
            onSnapshot?.(snapshot);
            return;
          }

          if (payload.type === "event" && payload.event === "loop_phase_transition" && payload.data) {
            const phaseEvent = payload.data as unknown as LoopPhaseTransitionPayload;
            setData((current) => {
              if (!current) {
                return current;
              }
              return {
                ...current,
                loop_visualization: phaseEvent.loop_visualization,
              };
            });
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
      ws?.close();
    };
  }, [onSnapshot]);

  return {
    data,
    socketConnected,
    socketError,
    wsCommand,
  };
}
