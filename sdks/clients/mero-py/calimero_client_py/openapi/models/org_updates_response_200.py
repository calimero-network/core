from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from typing import cast
from typing import cast, Union

if TYPE_CHECKING:
    from ..models.upgrade_policy import UpgradePolicy


T = TypeVar("T", bound="OrgUpdatesResponse200")


@_attrs_define
class OrgUpdatesResponse200:
    """
    Attributes:
        org (str):
        policy (UpgradePolicy): How an organization's owner and admins let an app's updates apply (`POST
            /orgs/{org}/upgrade-policy`, `POST /apps/{app}/upgrade-policy`); every field is answered; a request may leave
            any out (its default). `secondApprover` (default off): a red update needs two distinct owners or admins.
            `autoApply` (default `never`; `noDestinations`: a green update of an app whose unit has no destinations before
            or after it, so no data leaves Calimero) and `holdHours` (24 to 168, default 24): such an update applies on its
            own once `holdHours` have passed since the release was logged (its install, or its deployment's offer), after a
            restore point, and for an instance of a deployment in its rollout's wave.
        updates (list[Any]):
        next_ (Union[None, str]):
    """

    org: str
    policy: "UpgradePolicy"
    updates: list[Any]
    next_: Union[None, str]
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.upgrade_policy import UpgradePolicy

        org = self.org

        policy = self.policy.to_dict()

        updates = self.updates

        next_: Union[None, str]
        next_ = self.next_

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "org": org,
                "policy": policy,
                "updates": updates,
                "next": next_,
            }
        )

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.upgrade_policy import UpgradePolicy

        d = dict(src_dict)
        org = d.pop("org")

        policy = UpgradePolicy.from_dict(d.pop("policy"))

        updates = cast(list[Any], d.pop("updates"))

        def _parse_next_(data: object) -> Union[None, str]:
            if data is None:
                return data
            return cast(Union[None, str], data)

        next_ = _parse_next_(d.pop("next"))

        org_updates_response_200 = cls(
            org=org,
            policy=policy,
            updates=updates,
            next_=next_,
        )

        org_updates_response_200.additional_properties = d
        return org_updates_response_200

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
