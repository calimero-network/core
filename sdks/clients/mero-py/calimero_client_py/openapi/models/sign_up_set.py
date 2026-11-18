from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast

if TYPE_CHECKING:
    from ..models.governance import Governance
    from ..models.sign_up_state import SignUpState


T = TypeVar("T", bound="SignUpSet")


@_attrs_define
class SignUpSet:
    """The cluster's governance and sign-up policy after the change.

    Attributes:
        governance (Governance): Who decides what on this cluster: release approvals, app upgrades and outbound
            approvals (`owner`: each app's owner; `operator`: the operator), and, left out at their defaults, `deployments`
            (`open`), `deployers` (`operator`) and `signUp` (`open`).
        sign_up (SignUpState): The sign-up policy and what it implies: whether organization invites and cluster sign-up
            invites admit a new account, and whether one is required.
    """

    governance: "Governance"
    sign_up: "SignUpState"
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.governance import Governance
        from ..models.sign_up_state import SignUpState

        governance = self.governance.to_dict()

        sign_up = self.sign_up.to_dict()

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "governance": governance,
                "signUp": sign_up,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.governance import Governance
        from ..models.sign_up_state import SignUpState

        d = dict(src_dict)
        governance = Governance.from_dict(d.pop("governance"))

        sign_up = SignUpState.from_dict(d.pop("signUp"))

        sign_up_set = cls(
            governance=governance,
            sign_up=sign_up,
        )

        sign_up_set.additional_properties = d
        return sign_up_set

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
