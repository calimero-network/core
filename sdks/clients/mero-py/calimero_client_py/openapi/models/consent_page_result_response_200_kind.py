from enum import Enum


class ConsentPageResultResponse200Kind(str, Enum):
    ACCEPTINVITE = "acceptInvite"
    CREATEINVITE = "createInvite"
    CREATEORG = "createOrg"
    DELEGATE = "delegate"
    DELETEORG = "deleteOrg"
    INSTALL = "install"
    LEAVEORG = "leaveOrg"
    OUTBOUND = "outbound"
    RELEASE = "release"
    REMOVEMEMBER = "removeMember"
    REMOVEMEMBERTEAMS = "removeMemberTeams"
    RENAMEORG = "renameOrg"
    REVOKEINVITE = "revokeInvite"
    SETALLAPPS = "setAllApps"
    SETCAPABILITIES = "setCapabilities"
    SETEGRESS = "setEgress"
    SETMEMBERROLE = "setMemberRole"
    SETMEMBERTEAMS = "setMemberTeams"
    UPDATEPOLICY = "updatePolicy"
    UPDATES = "updates"
    UPGRADE = "upgrade"
    UPGRADEUNIT = "upgradeUnit"

    def __str__(self) -> str:
        return str(self.value)
