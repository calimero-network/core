from collections.abc import Mapping
from typing import Any, TypeVar, Optional, BinaryIO, TextIO, TYPE_CHECKING, Generator

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from ..types import UNSET, Unset

from ..types import UNSET, Unset
from typing import cast
from typing import cast, Union
from typing import Union

if TYPE_CHECKING:
    from ..models.app_policy_set_body_policy_type_0 import AppPolicySetBodyPolicyType0
    from ..models.confirm import Confirm


T = TypeVar("T", bound="AppPolicySetBody")


@_attrs_define
class AppPolicySetBody:
    """
    Attributes:
        policy (Union['AppPolicySetBodyPolicyType0', None, Unset]):
        confirm (Union[Unset, Confirm]): A passkey assertion over the challenge a `412 CONFIRM_REQUIRED` named: the same
            request sent again with it approves exactly that request.
    """

    policy: Union["AppPolicySetBodyPolicyType0", None, Unset] = UNSET
    confirm: Union[Unset, "Confirm"] = UNSET

    def to_dict(self) -> dict[str, Any]:
        from ..models.app_policy_set_body_policy_type_0 import (
            AppPolicySetBodyPolicyType0,
        )
        from ..models.confirm import Confirm

        policy: Union[None, Unset, dict[str, Any]]
        if isinstance(self.policy, Unset):
            policy = UNSET
        elif isinstance(self.policy, AppPolicySetBodyPolicyType0):
            policy = self.policy.to_dict()
        else:
            policy = self.policy

        confirm: Union[Unset, dict[str, Any]] = UNSET
        if not isinstance(self.confirm, Unset):
            confirm = self.confirm.to_dict()

        field_dict: dict[str, Any] = {}

        field_dict.update({})
        if policy is not UNSET:
            field_dict["policy"] = policy
        if confirm is not UNSET:
            field_dict["confirm"] = confirm

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.app_policy_set_body_policy_type_0 import (
            AppPolicySetBodyPolicyType0,
        )
        from ..models.confirm import Confirm

        d = dict(src_dict)

        def _parse_policy(
            data: object,
        ) -> Union["AppPolicySetBodyPolicyType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                policy_type_0 = AppPolicySetBodyPolicyType0.from_dict(data)

                return policy_type_0
            except:  # noqa: E722
                pass
            return cast(Union["AppPolicySetBodyPolicyType0", None, Unset], data)

        policy = _parse_policy(d.pop("policy", UNSET))

        _confirm = d.pop("confirm", UNSET)
        confirm: Union[Unset, Confirm]
        if isinstance(_confirm, Unset):
            confirm = UNSET
        else:
            confirm = Confirm.from_dict(_confirm)

        app_policy_set_body = cls(
            policy=policy,
            confirm=confirm,
        )

        return app_policy_set_body
