import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { afterEach, beforeEach } from "@jest/globals";

const originalMyraHome = process.env.MYRA_HOME;
let currentMyraHome: string | undefined;

beforeEach(async () => {
  currentMyraHome = await fs.mkdtemp(path.join(os.tmpdir(), "myra-sdk-test-"));
  process.env.MYRA_HOME = currentMyraHome;
});

afterEach(async () => {
  const myraHomeToDelete = currentMyraHome;
  currentMyraHome = undefined;

  if (originalMyraHome === undefined) {
    delete process.env.MYRA_HOME;
  } else {
    process.env.MYRA_HOME = originalMyraHome;
  }

  if (myraHomeToDelete) {
    await fs.rm(myraHomeToDelete, { recursive: true, force: true });
  }
});
