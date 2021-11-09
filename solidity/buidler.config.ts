import { usePlugin } from "@nomiclabs/buidler/config";

usePlugin("@nomiclabs/buidler-waffle");

export default {
    defaultNetwork: "buidlerevm",
    solc: {
        version: "0.6.12",
        optimizer: {
            enabled: true,
            runs: 10000,
        }
    },
    paths: {
        artifacts: "./build"
    }
};