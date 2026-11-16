from enum import Enum


class ConsentPageResultResponse200Kind(str, Enum):
    ACCEPTINVITE = "acceptInvite"
    CREATEINVITE = "createInvite"
    CREATEORG = "createOrg"
    DELETEORG = "deleteOrg"
    LEAVEORG = "leaveOrg"
    OUTBOUND = "outbound"
    RELEASE = "release"
    REMOVEMEMBER = "removeMember"
    REMOVEMEMBERTEAMS = "removeMemberTeams"
    RENAMEORG = "renameOrg"
    REVOKEINVITE = "revokeInvite"
    SETALLAPPS = "setAllApps"
    SETCAPABILITIES = "setCapabilities"
    SETMEMBERROLE = "setMemberRole"
    SETMEMBERTEAMS = "setMemberTeams"
    UPGRADE = "upgrade"

    def __str__(self) -> str:
        return str(self.value)
