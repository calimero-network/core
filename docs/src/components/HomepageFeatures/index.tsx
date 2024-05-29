import clsx from "clsx";
import Heading from "@theme/Heading";
import styles from "./styles.module.css";

type FeatureItem = {
  title: string;
  Svg: React.ComponentType<React.ComponentProps<"svg">>;
  description: JSX.Element;
};

const FeatureList: FeatureItem[] = [
  {
    title: "Robust framework",
    Svg: require("@site/static/home/home-framework.svg").default,
    description: (
      <>
        Quickly launch and configure nodes in our peer-to-peer network with
        user-friendly tools that minimize the complexity and technical
        challenges.
      </>
    ),
  },
  {
    title: "Comprehensive SDKs",
    Svg: require("@site/static/home/home-sdk.svg").default,
    description: (
      <>
        {" "}
        Jumpstart your decentralized apps with our SDKs, designed for easy
        integration into our robust peer-to-peer network.
      </>
    ),
  },
  {
    title: "Open Source project",
    Svg: require("@site/static/home/home-open-source.svg").default,
    description: (
      <>
        Contribute your code to help forge a platform that leads the way in
        innovation in the decentralized space
      </>
    ),
  },
];

function Feature({ title, Svg, description }: FeatureItem) {
  return (
    <div className={clsx("col col--4")}>
      <div className="text--center">
        <Svg className={styles.featureSvg} role="img" />
      </div>
      <div className="text--center padding-horiz--md">
        <Heading as="h3">{title}</Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures(): JSX.Element {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
