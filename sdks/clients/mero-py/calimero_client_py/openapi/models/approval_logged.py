from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.pending_approval import PendingApproval


T = TypeVar("T", bound="ApprovalLogged")


@_attrs_define
class ApprovalLogged:
    """`POST /apps/{app}/upgrade-approvals`: the first approval logged.

    Attributes:
        app (str):
        approval (PendingApproval): A first approval of a red update waiting for its second (`POST /apps/{app}/upgrade-
            approvals`): the app approved (`app`), its target, a unit's members' (`targets`, by app), who approved and when,
            when it lapses (`expiresAt`, a week on), and the class its approver was shown.
    """

    app: str
    approval: "PendingApproval"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.pending_approval import PendingApproval

        app = self.app

        approval = self.approval.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "app": app,
                "approval": approval,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.pending_approval import PendingApproval

        d = dict(src_dict)
        app = d.pop("app")

        approval = PendingApproval.from_dict(d.pop("approval"))

        approval_logged = cls(
            app=app,
            approval=approval,
        )

        approval_logged.additional_properties = d
        return approval_logged

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
