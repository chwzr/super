import "./landing.css";

import { CTASection } from "./components/CTASection";
import { Depth } from "./components/Depth";
import { DiamondStage } from "./components/DiamondStage";
import { Explain } from "./components/Explain";
import { Footer } from "./components/Footer";
import { Hero } from "./components/Hero";
import { Nav } from "./components/Nav";
import { Phases } from "./components/Phases";
import { Workflow } from "./components/Workflow";

export function LandingPage() {
  return (
    <div className="landing-root">
      <Nav />
      <Hero />
      <DiamondStage />
      <Phases />
      <Workflow />
      <Explain />
      <Depth />
      <CTASection />
      <Footer />
    </div>
  );
}
