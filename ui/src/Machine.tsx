import { Cpu, HardDrive, MemoryStick } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type HostInfo } from "./api";

export interface MachineSize {
  cpus: number;
  ram: number;
  disk: number;
}

/** Sizes that fit common uses; IDE backends and builds want the larger ones. */
const PRESETS = [
  { label: "Light", note: "a shell, small scripts", cpus: 2, ram: 2048 },
  { label: "Standard", note: "one agent, a web project", cpus: 2, ram: 4096 },
  { label: "Roomy", note: "builds, Docker, code-server", cpus: 4, ram: 8192 },
  { label: "Heavy", note: "JetBrains backend, big builds", cpus: 8, ram: 16384 },
];

const gib = (mib: number) => (mib % 1024 === 0 ? `${mib / 1024}` : (mib / 1024).toFixed(1));

function Slider(props: { icon: React.ReactNode; label: string; value: number; min: number; max: number; step: number; text: string; hint?: string; onChange: (value: number) => void }) {
  const share = ((Math.min(props.value, props.max) - props.min) / (props.max - props.min)) * 100;
  return (
    <label className="slider">
      <span className="slider-head">
        {props.icon}
        <span className="grow">{props.label}</span>
        <output>{props.text}</output>
      </span>
      <input
        type="range"
        min={props.min}
        max={props.max}
        step={props.step}
        value={Math.min(props.value, props.max)}
        onChange={(e) => props.onChange(+e.target.value)}
        style={{ "--share": `${share}%` } as React.CSSProperties}
      />
      {props.hint && <small className="muted">{props.hint}</small>}
    </label>
  );
}

/**
 * CPUs, memory and disk of a sandbox as sliders, bounded by what this computer has.
 * `minDisk` is the size a disk already has: disks can only grow.
 */
export function MachineControls(props: { value: MachineSize; minDisk?: number; onChange: (next: MachineSize) => void }) {
  const { value, onChange } = props;
  const [host, setHost] = useState<HostInfo | null>(null);
  useEffect(() => {
    api.host().then(setHost, () => {});
  }, []);
  const maxCpus = Math.max(host?.cpus ?? 8, value.cpus);
  const maxRam = Math.max(Math.floor((host?.ram_mib ?? 32768) / 1024) * 1024, value.ram, 2048);
  const minDisk = props.minDisk ?? 4;
  const presets = PRESETS.filter((preset) => preset.cpus <= maxCpus && preset.ram <= maxRam);

  return (
    <div className="machine">
      <div className="presets">
        {presets.map((preset) => (
          <button key={preset.label} type="button" className={value.cpus === preset.cpus && value.ram === preset.ram ? "picked" : ""} onClick={() => onChange({ ...value, cpus: preset.cpus, ram: preset.ram })}>
            <strong>{preset.label}</strong>
            <small>
              {preset.cpus} CPUs · {preset.ram / 1024} GiB
            </small>
            <small>{preset.note}</small>
          </button>
        ))}
      </div>
      <Slider
        icon={<Cpu size={16} aria-hidden="true" />}
        label="CPUs"
        value={value.cpus}
        min={1}
        max={maxCpus}
        step={1}
        text={`${value.cpus}`}
        hint={host ? `This computer has ${host.cpus}. A sandbox only uses them while it works.` : undefined}
        onChange={(cpus) => onChange({ ...value, cpus })}
      />
      <Slider
        icon={<MemoryStick size={16} aria-hidden="true" />}
        label="Memory"
        value={value.ram}
        min={512}
        max={maxRam}
        step={512}
        text={`${gib(value.ram)} GiB`}
        hint={host?.ram_mib ? `This computer has ${gib(Math.round(host.ram_mib / 512) * 512)} GiB. Leave some for everything else.` : undefined}
        onChange={(ram) => onChange({ ...value, ram })}
      />
      <Slider
        icon={<HardDrive size={16} aria-hidden="true" />}
        label="Disk"
        value={value.disk}
        min={minDisk}
        max={Math.max(200, value.disk)}
        step={1}
        text={`${value.disk} GiB`}
        hint={props.minDisk ? "A disk can only grow. It takes space on this computer only as it fills." : "Can be enlarged later. It takes space on this computer only as it fills."}
        onChange={(disk) => onChange({ ...value, disk })}
      />
    </div>
  );
}

/** The two ways to run a sandbox, with what each means in practice. */
export function BackendChoice(props: { value: string; onChange: (backend: string) => void }) {
  const options = [
    { id: "krun", name: "krun", note: "Light and fast to start. Shares folders with this computer. The usual choice." },
    { id: "qemu", name: "qemu", note: "A full virtual machine with the system's own kernel. Cannot share folders yet." },
  ];
  return (
    <div className="choices">
      {options.map((option) => (
        <label key={option.id} className={props.value === option.id ? "choice picked" : "choice"}>
          <input type="radio" name="backend" checked={props.value === option.id} onChange={() => props.onChange(option.id)} />
          <span>
            <strong>{option.name}</strong>
            <small>{option.note}</small>
          </span>
        </label>
      ))}
    </div>
  );
}
