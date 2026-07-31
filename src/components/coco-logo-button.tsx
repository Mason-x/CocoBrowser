"use client";

import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { cn } from "@/lib/utils";
import { Logo } from "./icons/logo";
import type { AppPage } from "./rail-nav";

const CLICK_THRESHOLD = 5;
const CLICK_WINDOW_MS = 2000;
const GRAVITY = 2200;
const BOUNCE_DAMPING = 0.6;
const INITIAL_HORIZONTAL_SPEED = 350;
const SPIN_SPEED = 720;
const MIN_BOUNCE_VELOCITY = 60;
const LOGO_HIDDEN_KEY = "coco-logo-hidden";

function useLogoEasterEgg({
  currentPage,
  onNavigate,
}: {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
}) {
  const clickTimestamps = useRef<number[]>([]);
  const [isPressed, setIsPressed] = useState(false);
  const [wobbleKey, setWobbleKey] = useState(0);
  const [isFalling, setIsFalling] = useState(false);
  /**
   * Click count toward the bounce trigger while the user is on the profiles
   * page. Capped at 4: each click here grows the logo by 25%, so step 4 has
   * doubled the original size. Click 5 fires `triggerFall` and resets.
   */
  const [growStep, setGrowStep] = useState(0);
  const resetTimeoutRef = useRef<number | null>(null);
  const [isHidden, setIsHidden] = useState(() => {
    try {
      return sessionStorage.getItem(LOGO_HIDDEN_KEY) === "1";
    } catch {
      return false;
    }
  });
  const logoRef = useRef<HTMLButtonElement>(null);
  const animFrameRef = useRef<number>(0);

  const triggerFall = useCallback(() => {
    const el = logoRef.current;
    if (!el || isFalling) return;
    setIsFalling(true);

    const rect = el.getBoundingClientRect();
    const startX = rect.left;
    const startY = rect.top;

    const clone = el.cloneNode(true) as HTMLElement;
    clone.style.position = "fixed";
    clone.style.left = `${startX}px`;
    clone.style.top = `${startY}px`;
    clone.style.zIndex = "9999";
    clone.style.pointerEvents = "none";
    clone.style.margin = "0";
    document.body.appendChild(clone);
    el.style.visibility = "hidden";

    let x = 0;
    let y = 0;
    let vy = -500;
    // Roll right first, bounce off the right wall, then escape the left.
    let vx = INITIAL_HORIZONTAL_SPEED;
    let rotation = 0;
    let lastTime = performance.now();

    const animate = (time: number) => {
      const dt = Math.min((time - lastTime) / 1000, 0.05);
      lastTime = time;

      // Read live so a mid-animation window resize moves the floor/wall.
      const floorY = window.innerHeight;
      const rightWall = window.innerWidth;

      vy += GRAVITY * dt;
      x += vx * dt;
      y += vy * dt;
      rotation += SPIN_SPEED * dt * (vx > 0 ? 1 : -1);

      const currentBottom = startY + y + rect.height;
      if (currentBottom >= floorY && vy > 0) {
        y = floorY - startY - rect.height;
        vy =
          Math.abs(vy) > MIN_BOUNCE_VELOCITY
            ? -Math.abs(vy) * BOUNCE_DAMPING
            : -MIN_BOUNCE_VELOCITY * 3;
      }

      // Right-wall bounce: hit, reverse horizontal velocity (with a tiny
      // damping), and keep rolling. Left wall has no bounce — the coco
      // exits the window off the left edge.
      const currentRight = startX + x + rect.width;
      if (currentRight >= rightWall && vx > 0) {
        x = rightWall - startX - rect.width;
        vx = -Math.abs(vx) * 0.9;
      }

      clone.style.transform = `translate(${x}px, ${y}px) rotate(${rotation}deg)`;

      const offScreenLeft = startX + x + rect.width < -200;
      const offScreenBottom = startY + y > floorY + 100;
      const offScreenTop = startY + y + rect.height < -200;

      if (offScreenLeft || offScreenBottom || offScreenTop) {
        clone.remove();
        try {
          sessionStorage.setItem(LOGO_HIDDEN_KEY, "1");
        } catch {
          // ignore — sessionStorage unavailable in some Tauri WebViews
        }
        setIsHidden(true);
        setIsFalling(false);
        return;
      }
      animFrameRef.current = requestAnimationFrame(animate);
    };
    animFrameRef.current = requestAnimationFrame(animate);
  }, [isFalling]);

  useEffect(() => {
    return () => {
      if (animFrameRef.current) cancelAnimationFrame(animFrameRef.current);
    };
  }, []);

  const handleClick = useCallback(() => {
    if (isFalling || isHidden) return;

    // First behaviour: any click from elsewhere in the app just routes the
    // user back to the profiles list. Growing the coco requires the user
    // to already be home — that keeps the easter egg from accidentally
    // firing during normal navigation.
    if (currentPage !== "profiles") {
      onNavigate("profiles");
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
      return;
    }

    const now = Date.now();
    clickTimestamps.current = clickTimestamps.current.filter(
      (t) => now - t < CLICK_WINDOW_MS,
    );
    clickTimestamps.current.push(now);

    if (clickTimestamps.current.length >= CLICK_THRESHOLD) {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
      triggerFall();
    } else {
      setGrowStep(
        Math.min(clickTimestamps.current.length, CLICK_THRESHOLD - 1),
      );
      setWobbleKey((k) => k + 1);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
      resetTimeoutRef.current = window.setTimeout(() => {
        clickTimestamps.current = [];
        setGrowStep(0);
        resetTimeoutRef.current = null;
      }, CLICK_WINDOW_MS);
    }
  }, [currentPage, isFalling, isHidden, onNavigate, triggerFall]);

  // Leaving the profiles page mid-streak cancels growth so we never end up
  // with an outsized logo when the user returns later.
  useEffect(() => {
    if (currentPage !== "profiles") {
      clickTimestamps.current = [];
      setGrowStep(0);
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
        resetTimeoutRef.current = null;
      }
    }
  }, [currentPage]);

  useEffect(() => {
    return () => {
      if (resetTimeoutRef.current !== null) {
        window.clearTimeout(resetTimeoutRef.current);
      }
    };
  }, []);

  return {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
  };
}

/**
 * The coco mark, and the easter egg behind it. Lives in the header rather than
 * the rail so the rail's first entry can line up with the table header beside
 * it; the rail is where it used to sit, which pushed every nav item down by the
 * height of a logo plus a divider.
 */
export function CocoLogoButton({
  currentPage,
  onNavigate,
}: {
  currentPage: AppPage;
  onNavigate: (page: AppPage) => void;
}) {
  const { t } = useTranslation();
  const {
    logoRef,
    isPressed,
    setIsPressed,
    wobbleKey,
    isFalling,
    isHidden,
    growStep,
    handleClick,
  } = useLogoEasterEgg({ currentPage, onNavigate });

  if (isHidden) {
    return <div className="size-7 shrink-0" />;
  }

  return (
    <button
      ref={logoRef}
      type="button"
      aria-label={t("header.cocoLogo")}
      className="grid size-7 shrink-0 cursor-pointer place-items-center rounded-md bg-transparent text-foreground select-none"
      onClick={handleClick}
      onPointerDown={() => {
        setIsPressed(true);
      }}
      onPointerUp={() => {
        setIsPressed(false);
      }}
      onPointerLeave={() => {
        setIsPressed(false);
      }}
    >
      {/* Inner wrapper survives clicks (no `key`) so the scale change
          animates smoothly across the wiggle layer's remounts. */}
      <span
        style={{
          transform: isPressed
            ? `scale(${(1 + growStep * 0.25) * 0.9})`
            : `scale(${1 + growStep * 0.25})`,
        }}
        className="inline-grid place-items-center transition-transform duration-300 ease-out will-change-transform"
      >
        <span
          key={wobbleKey}
          className={cn(
            "inline-grid place-items-center",
            !isFalling &&
              !isPressed &&
              wobbleKey > 0 &&
              "animate-[wiggle_0.3s_ease-in-out]",
          )}
        >
          <Logo className="size-5 will-change-transform" />
        </span>
      </span>
    </button>
  );
}
