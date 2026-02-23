import { SensorDetailClient } from "@/components/sensors/SensorDetailClient";

type SensorPageProps = {
  params: Promise<{ sensorName: string }>;
};

export default async function SensorDetailPage({ params }: SensorPageProps) {
  const resolved = await params;
  const sensorName = decodeURIComponent(resolved.sensorName);
  return <SensorDetailClient sensorName={sensorName} />;
}
