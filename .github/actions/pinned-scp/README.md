# Pinned SCP action

Vendored from [appleboy/scp-action](https://github.com/appleboy/scp-action/tree/917f8b81dfc1ccd331fef9e2d61bdc6c8be94634).
The action inputs and entrypoint are unchanged. The Docker base image is pinned
to its multi-platform manifest digest so replacing the upstream tag cannot
change the executable that receives deployment credentials.

When updating, review the upstream source and image together and retain LICENSE.
