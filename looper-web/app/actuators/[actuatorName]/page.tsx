import { ActuatorDetailClient } from "@/components/actuators/ActuatorDetailClient";

type ActuatorPageProps = {
  params: Promise<{ actuatorName: string }>;
};

export default async function ActuatorDetailPage({ params }: ActuatorPageProps) {
  const resolved = await params;
  const actuatorName = decodeURIComponent(resolved.actuatorName);
  return <ActuatorDetailClient actuatorName={actuatorName} />;
}
