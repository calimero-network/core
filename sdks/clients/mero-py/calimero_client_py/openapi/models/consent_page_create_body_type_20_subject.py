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
    from ..models.consent_page_create_body_type_20_subject_policy_type_0 import (
        ConsentPageCreateBodyType20SubjectPolicyType0,
    )


T = TypeVar("T", bound="ConsentPageCreateBodyType20Subject")


@_attrs_define
class ConsentPageCreateBodyType20Subject:
    """
    Attributes:
        org (Union[Unset, str]):
        app (Union[Unset, str]):
        policy (Union['ConsentPageCreateBodyType20SubjectPolicyType0', None, Unset]):
    """

    org: Union[Unset, str] = UNSET
    app: Union[Unset, str] = UNSET
    policy: Union["ConsentPageCreateBodyType20SubjectPolicyType0", None, Unset] = UNSET
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        from ..models.consent_page_create_body_type_20_subject_policy_type_0 import (
            ConsentPageCreateBodyType20SubjectPolicyType0,
        )

        org = self.org

        app = self.app

        policy: Union[None, Unset, dict[str, Any]]
        if isinstance(self.policy, Unset):
            policy = UNSET
        elif isinstance(self.policy, ConsentPageCreateBodyType20SubjectPolicyType0):
            policy = self.policy.to_dict()
        else:
            policy = self.policy

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update({})
        if org is not UNSET:
            field_dict["org"] = org
        if app is not UNSET:
            field_dict["app"] = app
        if policy is not UNSET:
            field_dict["policy"] = policy

        return field_dict

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        from ..models.consent_page_create_body_type_20_subject_policy_type_0 import (
            ConsentPageCreateBodyType20SubjectPolicyType0,
        )

        d = dict(src_dict)
        org = d.pop("org", UNSET)

        app = d.pop("app", UNSET)

        def _parse_policy(
            data: object,
        ) -> Union["ConsentPageCreateBodyType20SubjectPolicyType0", None, Unset]:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            try:
                if not isinstance(data, dict):
                    raise TypeError()
                policy_type_0 = ConsentPageCreateBodyType20SubjectPolicyType0.from_dict(
                    data
                )

                return policy_type_0
            except:  # noqa: E722
                pass
            return cast(
                Union["ConsentPageCreateBodyType20SubjectPolicyType0", None, Unset],
                data,
            )

        policy = _parse_policy(d.pop("policy", UNSET))

        consent_page_create_body_type_20_subject = cls(
            org=org,
            app=app,
            policy=policy,
        )

        consent_page_create_body_type_20_subject.additional_properties = d
        return consent_page_create_body_type_20_subject

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
